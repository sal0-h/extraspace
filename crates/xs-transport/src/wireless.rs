//! App-owned LAN connections. Android debugging is used only by the USB path.
//!
//! A temporary, high-entropy code authenticates the laptop's TLS certificate.
//! The QR contains the same code plus the certificate fingerprint and endpoints.
//! Pairing exchanges a separate random tablet credential over that verified TLS
//! connection. Subsequent connections pin the certificate and present that
//! credential. Discovery records are never a source of trust.

use std::collections::HashMap;
use std::net::{Ipv4Addr, TcpListener};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, ensure, Context, Result};
use mdns_sd::{ServiceDaemon, ServiceInfo};
use ring::{
    digest, hmac,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{broadcast, mpsc, Semaphore};
use tokio_rustls::{rustls, TlsAcceptor};
use tracing::{debug, warn};

use crate::{Adb, Device, DeviceState, Socket, Transport};

pub const SERVICE_TYPE: &str = "_extraspace._tcp.local.";
const PAIR_LIFETIME: Duration = Duration::from_secs(300);
const MAX_MESSAGE: usize = 8192;
const ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedDevice {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub enum WirelessEvent {
    Paired(PairedDevice),
    DevicesChanged,
}

#[derive(Clone)]
pub struct PairingOffer {
    pub code: String,
    pub qr: String,
}

#[derive(Serialize, Deserialize)]
struct Saved {
    certificate: String,
    private_key: String,
    peers: Vec<PairedDevice>,
}

struct Pairing {
    code: String,
    expires: Instant,
}

struct Pending {
    created: Instant,
    peer: PairedDevice,
    channels: HashMap<String, Socket>,
}

struct Inner {
    saved: Saved,
    pairing: Option<Pairing>,
    pending: HashMap<String, Pending>,
}

pub struct Wireless {
    path: PathBuf,
    fingerprint: String,
    name: String,
    port: u16,
    inner: Mutex<Inner>,
    listener: Mutex<Option<TcpListener>>,
    acceptor: TlsAcceptor,
    incoming_tx: mpsc::Sender<Transport>,
    incoming_rx: tokio::sync::Mutex<mpsc::Receiver<Transport>>,
    events: broadcast::Sender<WirelessEvent>,
}

impl Wireless {
    pub fn new() -> Result<Arc<Self>> {
        let root = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .context("No user configuration directory")?
            .join("extraspace");
        Self::at(root)
    }

    fn at(root: PathBuf) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&root)?;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
        let path = root.join("wireless.json");
        let saved = match std::fs::read(&path) {
            Ok(data) => serde_json::from_slice::<Saved>(&data)
                .context("Cannot read saved Wi-Fi identity")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let rcgen::CertifiedKey { cert, signing_key } =
                    rcgen::generate_simple_self_signed(vec!["extraspace.local".into()])?;
                Saved {
                    certificate: hex::encode(cert.der()),
                    private_key: hex::encode(signing_key.serialize_der()),
                    peers: Vec::new(),
                }
            }
            Err(e) => return Err(e.into()),
        };
        let certificate = hex::decode(&saved.certificate)?;
        let fingerprint = sha256(&certificate);
        let key = rustls::pki_types::PrivatePkcs8KeyDer::from(hex::decode(&saved.private_key)?);
        let provider = rustls::crypto::ring::default_provider();
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(provider))
            .with_protocol_versions(&[&rustls::version::TLS13])?
            .with_no_client_auth()
            .with_single_cert(vec![certificate.into()], key.into())?;
        // A stable port also gives a saved endpoint a chance when multicast is
        // filtered. Discovery and QR pairing carry the actual port if occupied.
        let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 47471))
            .or_else(|_| TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let name = std::fs::read_to_string("/etc/hostname")
            .unwrap_or_else(|_| "My computer".into())
            .trim()
            .chars()
            .take(60)
            .collect();
        let (incoming_tx, incoming_rx) = mpsc::channel(1);
        let (events, _) = broadcast::channel(16);
        let this = Arc::new(Self {
            path,
            fingerprint,
            name,
            port,
            inner: Mutex::new(Inner {
                saved,
                pairing: None,
                pending: HashMap::new(),
            }),
            listener: Mutex::new(Some(listener)),
            acceptor: TlsAcceptor::from(Arc::new(config)),
            incoming_tx,
            incoming_rx: tokio::sync::Mutex::new(incoming_rx),
            events,
        });
        this.save(&this.inner.lock().unwrap().saved)?;
        Ok(this)
    }

    fn save(&self, saved: &Saved) -> Result<()> {
        let temporary = self.path.with_extension("json.new");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true).mode(0o600);
        let mut file = options.open(&temporary)?;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
        std::io::Write::write_all(&mut file, &serde_json::to_vec(saved)?)?;
        file.sync_all()?;
        std::fs::rename(temporary, &self.path)?;
        Ok(())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<WirelessEvent> {
        self.events.subscribe()
    }

    pub fn paired_devices(&self) -> Vec<PairedDevice> {
        self.inner.lock().unwrap().saved.peers.clone()
    }

    pub fn begin_pairing(&self) -> Result<PairingOffer> {
        let code: String = random::<20>()?
            .into_iter()
            .map(|b| ALPHABET[(b & 31) as usize] as char)
            .collect();
        let qr = json!({
            "extraspace": 1, "fingerprint": self.fingerprint,
            "port": self.port, "hosts": local_addresses(), "code": code,
        })
        .to_string();
        let formatted = code
            .as_bytes()
            .chunks(4)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect::<Vec<_>>()
            .join("-");
        self.inner.lock().unwrap().pairing = Some(Pairing {
            code,
            expires: Instant::now() + PAIR_LIFETIME,
        });
        Ok(PairingOffer {
            code: formatted,
            qr,
        })
    }

    pub fn cancel_pairing(&self) {
        self.inner.lock().unwrap().pairing = None;
    }

    pub fn forget(&self, id: &str) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let peers = inner.saved.peers.clone();
        inner.saved.peers.retain(|p| p.id != id);
        if let Err(e) = self.save(&inner.saved) {
            inner.saved.peers = peers;
            return Err(e);
        }
        inner.pending.retain(|_, p| p.peer.id != id);
        let _ = self.events.send(WirelessEvent::DevicesChanged);
        Ok(())
    }

    pub async fn accept(&self) -> Option<Transport> {
        self.incoming_rx.lock().await.recv().await
    }

    pub async fn serve(self: Arc<Self>) -> Result<()> {
        let listener = self
            .listener
            .lock()
            .unwrap()
            .take()
            .context("Wi-Fi listener already started")?;
        let listener = tokio::net::TcpListener::from_std(listener)?;
        let daemon = match ServiceDaemon::new() {
            Ok(daemon) => Some(daemon),
            Err(e) => {
                warn!(error = %e, "Discovery unavailable; QR and saved endpoints can still connect");
                None
            }
        };
        let _guard = daemon.clone().map(DiscoveryGuard);
        let mut advertised = Vec::new();
        let mut refresh = tokio::time::interval(Duration::from_secs(5));
        let limit = Arc::new(Semaphore::new(16));
        loop {
            tokio::select! {
                _ = refresh.tick() => {
                    let ips = local_addresses();
                    if ips != advertised && !ips.is_empty() {
                        let service = ServiceInfo::new(
                            SERVICE_TYPE, &format!("Extraspace-{}", &self.fingerprint[..12]),
                            &format!("extraspace-{}.local.", &self.fingerprint[..12]),
                            ips.join(",").as_str(), self.port,
                            [("id", self.fingerprint.as_str()), ("name", self.name.as_str()), ("version", "1")].as_slice(),
                        )?;
                        if let Some(daemon) = &daemon {
                            if let Err(e) = daemon.register(service) { warn!(error = %e, "Wi-Fi discovery unavailable"); }
                        }
                        advertised = ips;
                    }
                    self.inner.lock().unwrap().pending.retain(|_, p| p.created.elapsed() < Duration::from_secs(15));
                }
                accepted = listener.accept() => {
                    let (stream, _) = accepted?;
                    let Ok(permit) = Arc::clone(&limit).try_acquire_owned() else { continue };
                    let _ = stream.set_nodelay(true);
                    let this = Arc::clone(&self);
                    tokio::spawn(async move {
                        let _permit = permit;
                        if let Ok(Err(e)) = tokio::time::timeout(Duration::from_secs(10), this.handle(stream)).await {
                            debug!(error = %e, "Wi-Fi handshake rejected");
                        }
                    });
                }
            }
        }
    }

    async fn handle(&self, socket: tokio::net::TcpStream) -> Result<()> {
        let mut stream = self.acceptor.accept(socket).await?;
        let hello = read_json(&mut stream).await?;
        match field(&hello, "kind")? {
            "pair_hello" => {
                let client_nonce = field(&hello, "nonce")?;
                ensure!(
                    hex::decode(client_nonce)?.len() == 32,
                    "Invalid pairing nonce"
                );
                let code = {
                    let inner = self.inner.lock().unwrap();
                    let pair = inner
                        .pairing
                        .as_ref()
                        .context("Choose Pair tablet on the computer first")?;
                    ensure!(Instant::now() < pair.expires, "Pairing code expired");
                    pair.code.clone()
                };
                let server_nonce = hex::encode(random::<32>()?);
                let transcript = format!("{}:{client_nonce}:{server_nonce}", self.fingerprint);
                let server_proof = proof(&code, "server", &transcript);
                write_json(
                    &mut stream,
                    &json!({"kind":"pair_challenge", "nonce":server_nonce, "proof":server_proof}),
                )
                .await?;
                let finish = read_json(&mut stream).await?;
                ensure!(
                    field(&finish, "kind")? == "pair_finish",
                    "Invalid pairing response"
                );
                verify_proof(&code, "client", &transcript, field(&finish, "proof")?)?;
                let token = field(&finish, "token")?;
                ensure!(hex::decode(token)?.len() == 32, "Invalid device credential");
                let name: String = field(&finish, "name")?
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(80)
                    .collect();
                ensure!(!name.is_empty(), "Missing device name");
                let peer = PairedDevice {
                    id: sha256(token.as_bytes()),
                    name,
                };
                {
                    let mut inner = self.inner.lock().unwrap();
                    let pairing = inner.pairing.as_ref().context("Pairing was cancelled")?;
                    ensure!(
                        pairing.code == code && Instant::now() < pairing.expires,
                        "Pairing code expired or was used"
                    );
                    ensure!(
                        inner.saved.peers.len() < 32,
                        "Forget a device before pairing another"
                    );
                    inner.saved.peers.push(peer.clone());
                    if let Err(e) = self.save(&inner.saved) {
                        inner.saved.peers.pop();
                        return Err(e);
                    }
                    inner.pairing = None;
                }
                write_json(
                    &mut stream,
                    &json!({"kind":"paired", "id":self.fingerprint, "name":self.name}),
                )
                .await?;
                let _ = self.events.send(WirelessEvent::Paired(peer));
            }
            "channel" => {
                let id = sha256(field(&hello, "token")?.as_bytes());
                let session = field(&hello, "session")?;
                ensure!(
                    session.len() == 32 && hex::decode(session).is_ok(),
                    "Invalid session identifier"
                );
                let channel = field(&hello, "channel")?;
                ensure!(
                    ["control", "video", "camera"].contains(&channel),
                    "Invalid channel"
                );
                let key = format!("{id}:{session}");
                {
                    let inner = self.inner.lock().unwrap();
                    ensure!(
                        inner.saved.peers.iter().any(|p| p.id == id),
                        "This tablet is not paired"
                    );
                    ensure!(
                        inner.pending.len() < 8 || inner.pending.contains_key(&key),
                        "Too many pending sessions"
                    );
                    ensure!(
                        !inner
                            .pending
                            .get(&key)
                            .is_some_and(|p| p.channels.contains_key(channel)),
                        "Duplicate channel"
                    );
                }
                write_json(&mut stream, &json!({"kind":"ready"})).await?;
                let transport = {
                    let mut inner = self.inner.lock().unwrap();
                    let peer = inner
                        .saved
                        .peers
                        .iter()
                        .find(|p| p.id == id)
                        .cloned()
                        .context("Pairing revoked")?;
                    let pending = inner.pending.entry(key.clone()).or_insert_with(|| Pending {
                        created: Instant::now(),
                        peer,
                        channels: HashMap::new(),
                    });
                    ensure!(!pending.channels.contains_key(channel), "Duplicate channel");
                    pending
                        .channels
                        .insert(channel.to_owned(), Box::new(stream));
                    if pending.channels.len() == 3 {
                        let mut pending = inner.pending.remove(&key).unwrap();
                        Some(Transport {
                            device: Device {
                                serial: format!("wifi:{id}"),
                                state: DeviceState::Ready,
                                model: Some(pending.peer.name),
                            },
                            control: pending.channels.remove("control").unwrap(),
                            video: pending.channels.remove("video").unwrap(),
                            camera: pending.channels.remove("camera").unwrap(),
                            adb: Adb::none(),
                            simulated: true,
                        })
                    } else {
                        None
                    }
                };
                if let Some(transport) = transport {
                    let _ = self.incoming_tx.try_send(transport);
                }
            }
            _ => bail!("Unknown connection request"),
        }
        Ok(())
    }
}

struct DiscoveryGuard(ServiceDaemon);
impl Drop for DiscoveryGuard {
    fn drop(&mut self) {
        let _ = self.0.shutdown();
    }
}

fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| anyhow::anyhow!("Secure randomness unavailable"))?;
    Ok(bytes)
}

fn sha256(data: &[u8]) -> String {
    hex::encode(digest::digest(&digest::SHA256, data))
}

fn proof(code: &str, role: &str, transcript: &str) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, code.as_bytes());
    hex::encode(hmac::sign(
        &key,
        format!("extraspace-pair-{role}-v1:{transcript}").as_bytes(),
    ))
}

fn verify_proof(code: &str, role: &str, transcript: &str, value: &str) -> Result<()> {
    let key = hmac::Key::new(hmac::HMAC_SHA256, code.as_bytes());
    hmac::verify(
        &key,
        format!("extraspace-pair-{role}-v1:{transcript}").as_bytes(),
        &hex::decode(value)?,
    )
    .map_err(|_| anyhow::anyhow!("Pairing code did not match"))
}

fn field<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
    value
        .get(name)
        .and_then(Value::as_str)
        .with_context(|| format!("Missing {name}"))
}

fn local_addresses() -> Vec<String> {
    let mut ips: Vec<String> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|i| match i.ip() {
            std::net::IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_link_local() => {
                Some(ip.to_string())
            }
            _ => None,
        })
        .collect();
    ips.sort();
    ips.dedup();
    ips
}

async fn read_json<S: AsyncRead + Unpin>(stream: &mut S) -> Result<Value> {
    let length = stream.read_u32().await? as usize;
    ensure!(
        (1..=MAX_MESSAGE).contains(&length),
        "Invalid handshake size"
    );
    let mut data = vec![0; length];
    stream.read_exact(&mut data).await?;
    Ok(serde_json::from_slice(&data)?)
}

async fn write_json<S: AsyncWrite + Unpin>(stream: &mut S, value: &Value) -> Result<()> {
    let data = serde_json::to_vec(value)?;
    stream.write_u32(data.len() as u32).await?;
    stream.write_all(&data).await?;
    stream.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_proof_binds_both_nonces_the_certificate_and_direction() {
        let code = "ABCDE12345ABCDE12345";
        let transcript = "certificate:client-nonce:server-nonce";
        let valid = proof(code, "server", transcript);
        assert!(verify_proof(code, "server", transcript, &valid).is_ok());
        for changed in [
            "other:client-nonce:server-nonce",
            "certificate:other:server-nonce",
            "certificate:client-nonce:other",
        ] {
            assert!(verify_proof(code, "server", changed, &valid).is_err());
        }
        assert!(verify_proof("wrong", "server", transcript, &valid).is_err());
        assert!(verify_proof(code, "client", transcript, &valid).is_err());
    }

    async fn tls_client(
        wireless: &Arc<Wireless>,
    ) -> tokio_rustls::client::TlsStream<tokio::net::TcpStream> {
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(
                hex::decode(&wireless.inner.lock().unwrap().saved.certificate)
                    .unwrap()
                    .into(),
            )
            .unwrap();
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
        let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
        let stream = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, wireless.port))
            .await
            .unwrap();
        connector
            .connect(
                rustls::pki_types::ServerName::try_from("extraspace.local").unwrap(),
                stream,
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn trust_survives_restart_and_revocation_rejects_every_channel() {
        let dir = tempfile::tempdir().unwrap();
        let wireless = Wireless::at(dir.path().into()).unwrap();
        let listener =
            tokio::net::TcpListener::from_std(wireless.listener.lock().unwrap().take().unwrap())
                .unwrap();
        let server = Arc::clone(&wireless);
        let task = tokio::spawn(async move {
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                let server = Arc::clone(&server);
                tokio::spawn(async move {
                    let _ = server.handle(socket).await;
                });
            }
        });

        // Merely knowing the network endpoint never authorizes a channel.
        let token = hex::encode(random::<32>().unwrap());
        let request = |channel: &str| json!({"kind":"channel", "channel":channel, "token":token, "session":"0123456789abcdef0123456789abcdef"});
        let mut stranger = tls_client(&wireless).await;
        write_json(&mut stranger, &request("control"))
            .await
            .unwrap();
        assert!(read_json(&mut stranger).await.is_err());

        let offer = wireless.begin_pairing().unwrap();
        let code = offer.code.replace('-', "");
        let mut pair = tls_client(&wireless).await;
        let client_nonce = hex::encode(random::<32>().unwrap());
        write_json(
            &mut pair,
            &json!({"kind":"pair_hello", "nonce":client_nonce}),
        )
        .await
        .unwrap();
        let challenge = read_json(&mut pair).await.unwrap();
        let transcript = format!(
            "{}:{client_nonce}:{}",
            wireless.fingerprint,
            challenge["nonce"].as_str().unwrap()
        );
        verify_proof(
            &code,
            "server",
            &transcript,
            challenge["proof"].as_str().unwrap(),
        )
        .unwrap();
        write_json(&mut pair, &json!({"kind":"pair_finish", "name":"My tablet", "token":token, "proof":proof(&code, "client", &transcript)})).await.unwrap();
        assert_eq!(read_json(&mut pair).await.unwrap()["kind"], "paired");
        assert!(wireless.inner.lock().unwrap().pairing.is_none());
        let saved = Wireless::at(dir.path().into()).unwrap();
        assert_eq!(saved.fingerprint, wireless.fingerprint);
        assert_eq!(saved.paired_devices().len(), 1);
        assert_eq!(
            std::fs::metadata(&wireless.path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        let mut channels = Vec::new();
        for channel in ["control", "video", "camera"] {
            let mut socket = tls_client(&wireless).await;
            write_json(&mut socket, &request(channel)).await.unwrap();
            assert_eq!(read_json(&mut socket).await.unwrap()["kind"], "ready");
            channels.push(socket);
        }
        let mut transport = tokio::time::timeout(Duration::from_secs(2), wireless.accept())
            .await
            .unwrap()
            .unwrap();
        channels[0].write_all(b"trusted").await.unwrap();
        channels[0].flush().await.unwrap();
        let mut message = [0; 7];
        transport.control.read_exact(&mut message).await.unwrap();
        assert_eq!(&message, b"trusted");

        wireless.forget(&sha256(token.as_bytes())).unwrap();
        assert!(Wireless::at(dir.path().into())
            .unwrap()
            .paired_devices()
            .is_empty());
        for channel in ["control", "video", "camera"] {
            let mut revoked = tls_client(&wireless).await;
            write_json(&mut revoked, &request(channel)).await.unwrap();
            assert!(read_json(&mut revoked).await.is_err());
        }
        task.abort();
    }
}
