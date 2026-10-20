//! Real-tablet measurements using the production engine and saved pairing.
//! Close the desktop app first. See --help; stdin accepts mark/snapshot/quit.

use std::io::{self, BufRead};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use serde_json::json;
use xs_core::{
    BitrateBounds, Command, DisplayMode, Event, SessionConfig, State, TransportPreference,
};

#[derive(Debug)]
struct Options {
    native: bool,
    transport: TransportPreference,
    seconds: u64,
    fixed_kbps: Option<u32>,
    serial: Option<String>,
    label: String,
}

impl Options {
    fn parse() -> anyhow::Result<Self> {
        let mut options = Self {
            native: false,
            transport: TransportPreference::Auto,
            seconds: 90,
            fixed_kbps: None,
            serial: None,
            label: String::new(),
        };
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            if arg == "--help" {
                println!(
                    "resolution_bench [--resolution current|native] [--transport auto|wifi|usb] \
                          [--seconds 90] [--fixed-kbps N] [--serial USB_SERIAL] [--label NAME]\n\n\
                          Duration starts at the first stream, with a 30 s connection timeout.\n\
                          Camera is disabled. Settings and saved pairing are not rewritten.\n\
                          stdin: mark NAME | snapshot NEW_DIRECTORY | quit"
                );
                std::process::exit(0);
            }
            let value = args
                .next()
                .with_context(|| format!("missing value for {arg}"))?;
            match arg.as_str() {
                "--resolution" => {
                    options.native = match value.as_str() {
                        "native" => true,
                        "current" => false,
                        _ => bail!("resolution must be current or native"),
                    }
                }
                "--transport" => {
                    options.transport = match value.as_str() {
                        "auto" => TransportPreference::Auto,
                        "wifi" => TransportPreference::Wifi,
                        "usb" => TransportPreference::Adb,
                        _ => bail!("transport must be auto, wifi, or usb"),
                    }
                }
                "--seconds" => options.seconds = value.parse()?,
                "--fixed-kbps" => options.fixed_kbps = Some(value.parse()?),
                "--serial" => options.serial = Some(value),
                "--label" => options.label = value,
                _ => bail!("unknown option {arg}"),
            }
        }
        anyhow::ensure!(options.seconds > 0, "seconds must be positive");
        anyhow::ensure!(options.fixed_kbps != Some(0), "bitrate must be positive");
        anyhow::ensure!(
            options.serial.is_none() || options.transport == TransportPreference::Adb,
            "--serial is only valid with --transport usb"
        );
        Ok(options)
    }
}

fn main() -> anyhow::Result<()> {
    let options = Options::parse()?;
    // Set before creating the runtime, GStreamer, or any worker threads.
    if let Some(serial) = &options.serial {
        std::env::set_var("ANDROID_SERIAL", serial);
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(options))
}

async fn run(options: Options) -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(io::stderr)
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,xs_core=debug,xs_video=debug".into()),
        )
        .init();
    if options.native {
        anyhow::ensure!(
            xs_core::scaled_modes_supported(),
            "native mode refused by the running-compositor compatibility check"
        );
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let config_path = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"))
        .join("extraspace/config.json");
    let settings: serde_json::Value = serde_json::from_slice(&std::fs::read(config_path)?)?;
    let bounds = if let Some(kbps) = options.fixed_kbps {
        BitrateBounds {
            min_kbps: kbps,
            max_kbps: kbps,
        }
    } else {
        BitrateBounds {
            min_kbps: settings["min_bitrate_kbps"].as_u64().unwrap_or(6000) as u32,
            max_kbps: settings["max_bitrate_kbps"].as_u64().unwrap_or(50000) as u32,
        }
    };
    let config = SessionConfig {
        transport: options.transport,
        native_resolution: options.native,
        mode: DisplayMode::Extend,
        scale: settings["scale"].as_f64().unwrap_or(1.5),
        framerate: settings["framerate"].as_u64().unwrap_or(60) as u32,
        bounds,
        camera_enabled: false,
        ..SessionConfig::default()
    };
    println!(
        "{}",
        json!({"event":"config", "label":options.label,
        "native":options.native, "transport":format!("{:?}",options.transport),
        "scale":config.scale, "framerate":config.framerate,
        "min_kbps":bounds.min_kbps, "max_kbps":bounds.max_kbps, "seconds":options.seconds})
    );
    let engine = xs_core::spawn(config);
    if options.transport == TransportPreference::Wifi && engine.wireless.is_none() {
        engine.shutdown().await;
        bail!("Wi-Fi server unavailable; close the desktop app before benchmarking");
    }
    let mut events = engine.subscribe();
    let (requests_tx, mut requests_rx) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines().map_while(Result::ok) {
            if requests_tx.send(line).is_err() {
                break;
            }
        }
    });
    engine.send(Command::Connect);
    let deadline = tokio::time::sleep(Duration::from_secs(30));
    tokio::pin!(deadline);
    let mut stream_started = None::<Instant>;
    let mut stdin_open = true;
    let result = loop {
        let elapsed = || stream_started.map(|s| s.elapsed().as_secs_f64());
        tokio::select! {
            _ = &mut deadline => break if stream_started.is_some() { Ok(()) } else { Err(anyhow::anyhow!("no real tablet stream within 30 seconds")) },
            request = requests_rx.recv(), if stdin_open => {
                match request {
                    None => stdin_open = false,
                    Some(request) if request == "quit" => break Ok(()),
                    Some(request) => {
                        if let Some(path) = request.strip_prefix("snapshot ") {
                            engine.send(Command::SaveVideoDiagnostic(PathBuf::from(path)));
                        } else if let Some(label) = request.strip_prefix("mark ") {
                            println!("{}", json!({"event":"mark", "elapsed_s":elapsed(), "label":label}));
                        }
                    }
                }
            }
            event = events.recv() => {
                let event = match event { Ok(event) => event, Err(e) => break Err(e.into()) };
                match event {
                    Event::State(state) => {
                        println!("{}", json!({"event":"state", "elapsed_s":elapsed(), "state":state}));
                        match state {
                            State::Streaming { width, height, encoder, connection, .. } => {
                                if (options.transport == TransportPreference::Adb && connection != "USB")
                                    || (options.transport == TransportPreference::Wifi && connection != "Wi-Fi") {
                                    break Err(anyhow::anyhow!("wrong transport: {connection}"));
                                }
                                if stream_started.is_none() {
                                    stream_started = Some(Instant::now());
                                    deadline.as_mut().reset(tokio::time::Instant::now() + Duration::from_secs(options.seconds));
                                }
                                println!("{}", json!({"event":"streaming", "width":width, "height":height,
                                    "encoder":encoder, "connection":connection}));
                            }
                            State::Failed { message } => break Err(anyhow::anyhow!(message)),
                            _ => {}
                        }
                    }
                    Event::Stats(stats) => {
                        println!("{}", json!({
                            "event":"stats", "elapsed_s":elapsed(),
                            "encoded":stats.frames_encoded, "decoded":stats.frames_decoded,
                            "dropped":stats.frames_dropped, "queue":stats.decode_queue_depth,
                            "kbps":stats.bitrate_kbps, "rtt_ms":stats.latency_ms, "fps":stats.fps,
                            "pacing":stats.pacing,
                        }));
                    }
                    Event::Warning(message) => eprintln!("{message}"),
                    _ => {}
                }
            }
        }
    };
    engine.shutdown().await;
    result?;
    anyhow::ensure!(
        stream_started.is_some(),
        "no real tablet stream was established"
    );
    Ok(())
}
