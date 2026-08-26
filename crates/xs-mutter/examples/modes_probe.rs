//! Checks that mutter accepts our `RecordVirtual` modes, without touching the screen.
//!
//! `RecordVirtual` only validates and stores the mode list; the virtual monitor
//! is not created until `RemoteDesktop.Session.Start`, which this never calls. So
//! this is safe to run on a live desktop.
//!
//! ```console
//! cargo run -p xs-mutter --example modes_probe
//! ```

use std::collections::HashMap;

use zbus::Connection;
use zvariant::{OwnedObjectPath, Structure, Value};

async fn try_modes(
    conn: &Connection,
    label: &str,
    modes: Vec<HashMap<&'static str, Value<'static>>>,
) -> anyhow::Result<()> {
    let rd: OwnedObjectPath = conn
        .call_method(
            Some("org.gnome.Mutter.RemoteDesktop"),
            "/org/gnome/Mutter/RemoteDesktop",
            Some("org.gnome.Mutter.RemoteDesktop"),
            "CreateSession",
            &(),
        )
        .await?
        .body()
        .deserialize()?;
    let rd_id: String = conn
        .call_method(
            Some("org.gnome.Mutter.RemoteDesktop"),
            rd.as_str(),
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.gnome.Mutter.RemoteDesktop.Session", "SessionId"),
        )
        .await?
        .body()
        .deserialize::<Value>()?
        .downcast_ref::<String>()?;

    let sc_props: HashMap<&str, Value<'_>> =
        HashMap::from([("remote-desktop-session-id", Value::from(rd_id.as_str()))]);
    let sc: OwnedObjectPath = conn
        .call_method(
            Some("org.gnome.Mutter.ScreenCast"),
            "/org/gnome/Mutter/ScreenCast",
            Some("org.gnome.Mutter.ScreenCast"),
            "CreateSession",
            &(sc_props,),
        )
        .await?
        .body()
        .deserialize()?;

    let props: HashMap<&str, Value<'_>> = HashMap::from([
        ("is-platform", Value::from(true)),
        ("cursor-mode", Value::from(2u32)),
        ("modes", Value::from(modes)),
    ]);
    let result = conn
        .call_method(
            Some("org.gnome.Mutter.ScreenCast"),
            sc.as_str(),
            Some("org.gnome.Mutter.ScreenCast.Session"),
            "RecordVirtual",
            &(props,),
        )
        .await;
    match &result {
        Ok(_) => println!("  {label:<28} ACCEPTED"),
        Err(e) => println!("  {label:<28} rejected: {e}"),
    }

    // Stop without ever starting: no virtual monitor was created.
    let _ = conn
        .call_method(
            Some("org.gnome.Mutter.RemoteDesktop"),
            rd.as_str(),
            Some("org.gnome.Mutter.RemoteDesktop.Session"),
            "Stop",
            &(),
        )
        .await;
    Ok(())
}

fn mode(
    w: u32,
    h: u32,
    preferred: bool,
    scale: Option<f64>,
) -> HashMap<&'static str, Value<'static>> {
    let mut m: HashMap<&'static str, Value<'static>> = HashMap::new();
    m.insert("size", Value::from(Structure::from((w, h))));
    m.insert("refresh-rate", Value::from(60.0f64));
    if preferred {
        m.insert("is-preferred", Value::from(true));
    }
    if let Some(scale) = scale {
        m.insert("preferred-scale", Value::from(scale));
    }
    m
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!(
        "scaled modes allowed here: {}",
        xs_mutter::scaled_modes_allowed()
    );

    let conn = Connection::session().await?;
    println!("probing RecordVirtual modes (no Start, so no monitor is created)");

    // The real thing: native-ish size with an exact 1.75x logical scale.
    try_modes(
        &conn,
        "2296x1428 @1.75 + fallbacks",
        vec![
            mode(2296, 1428, true, Some(1.75)),
            mode(2304, 1440, false, None),
            mode(1316, 822, false, None),
        ],
    )
    .await?;

    // Control: if our dict shape were wrong, this would fail on `size` instead of
    // on the missing preferred flag.
    try_modes(
        &conn,
        "no is-preferred (expect err)",
        vec![mode(2296, 1428, false, Some(1.75))],
    )
    .await?;

    // Control: a scale whose logical size is fractional.
    try_modes(
        &conn,
        "2304x1440 @1.75 (fractional)",
        vec![mode(2304, 1440, true, Some(1.75))],
    )
    .await?;
    Ok(())
}
