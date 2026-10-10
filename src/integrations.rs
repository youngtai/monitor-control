use crate::gui::Action;
use ashpd::desktop::{
    global_shortcuts::{GlobalShortcuts, NewShortcut},
    Session,
};
use async_channel::Sender;
use futures_util::StreamExt;
use std::sync::Arc;

#[derive(Clone, Copy)]
pub enum ShortcutCommand {
    Connect,
    Configure,
    Reconnect,
    Disable,
}

type ShortcutSession = (
    Arc<GlobalShortcuts>,
    Arc<Session<GlobalShortcuts>>,
    tokio::task::JoinHandle<()>,
);

pub fn start(tx: Sender<Action>) -> Sender<ShortcutCommand> {
    let (commands, rx) = async_channel::unbounded();
    let events = tx.clone();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("desktop event runtime");
        runtime.block_on(async move {
            let a = events.clone();
            tokio::spawn(async move {
                if let Err(e) = watch_monitors(a.clone()).await {
                    let _ = a
                        .send(Action::Notice(format!(
                            "Desktop monitor events unavailable: {e}"
                        )))
                        .await;
                }
            });
            let a = events.clone();
            tokio::spawn(async move {
                if let Err(e) = watch_resume(a.clone()).await {
                    let _ = a
                        .send(Action::Notice(format!("Resume events unavailable: {e}")))
                        .await;
                }
            });
            let mut session: Option<ShortcutSession> = None;
            while let Ok(command) = rx.recv().await {
                if matches!(command, ShortcutCommand::Disable) {
                    if let Some((_, s, task)) = session.take() {
                        task.abort();
                        let _ = s.close().await;
                    }
                    let _ = events
                        .send(Action::ShortcutStatus(
                            false,
                            "Keyboard shortcuts disabled".into(),
                        ))
                        .await;
                    continue;
                }
                if matches!(command, ShortcutCommand::Reconnect) {
                    if let Some((_, s, task)) = session.take() {
                        task.abort();
                        let _ = s.close().await;
                    }
                    // Give Shell its settings-change turn before re-grabbing.
                    tokio::time::sleep(std::time::Duration::from_millis(350)).await;
                }
                if session
                    .as_ref()
                    .is_some_and(|(_, _, task)| task.is_finished())
                {
                    if let Some((_, s, _)) = session.take() {
                        let _ = s.close().await;
                    }
                }
                if let Some((portal, s, _)) = &session {
                    if matches!(command, ShortcutCommand::Configure) {
                        if portal.version() < 2 {
                            let _ = events.send(Action::OpenShortcutSettings).await;
                            continue;
                        }
                        let result = portal
                            .configure_shortcuts(s, None, Default::default())
                            .await;
                        if let Err(e) = result {
                            let _ = events
                                .send(Action::Notice(format!("Cannot configure shortcuts: {e}")))
                                .await;
                        }
                    }
                    continue;
                }
                match connect_shortcuts(events.clone()).await {
                    Ok(connected) => session = Some(connected),
                    Err(e) => {
                        let _ = events
                            .send(Action::ShortcutStatus(
                                false,
                                format!("Keyboard shortcuts unavailable: {e}"),
                            ))
                            .await;
                    }
                }
            }
        });
    });
    std::thread::spawn(move || watch_udev(tx));
    commands
}
async fn shortcut_portal() -> Result<GlobalShortcuts, ashpd::Error> {
    let connection = zbus::Connection::session().await?;
    // Register the host application's identity on the SAME connection used
    // for the portal session, before any app-id-gated portal calls.
    let registry = zbus::Proxy::new(
        &connection,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.host.portal.Registry",
    )
    .await?;
    let options: std::collections::HashMap<&str, zbus::zvariant::Value<'_>> =
        std::collections::HashMap::new();
    let registered: zbus::Result<()> = registry
        .call("Register", &(crate::gui::APP_ID, options))
        .await;
    if let Err(error) = registered {
        // Older portal versions use automatic host identity detection.
        match &error {
            zbus::Error::MethodError(name, _, _)
                if name.as_str() == "org.freedesktop.DBus.Error.UnknownMethod"
                    || name.as_str() == "org.freedesktop.DBus.Error.UnknownInterface" => {}
            _ => return Err(error.into()),
        }
    }
    GlobalShortcuts::with_connection(connection).await
}

pub fn probe_shortcuts() -> Result<(), String> {
    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let portal = shortcut_portal().await.map_err(|e| e.to_string())?;
        let session = portal
            .create_session(Default::default())
            .await
            .map_err(|e| e.to_string())?;
        session.close().await.map_err(|e| e.to_string())?;
        println!(
            "Global shortcuts portal v{}: host identity registered; session created and closed",
            portal.version()
        );
        Ok(())
    })
}

async fn connect_shortcuts(tx: Sender<Action>) -> Result<ShortcutSession, ashpd::Error> {
    let portal = Arc::new(shortcut_portal().await?);
    let session = Arc::new(portal.create_session(Default::default()).await?);
    let mut activated = portal.receive_activated().await?;
    let mut changed = portal.receive_shortcuts_changed().await?;

    let bound = portal
        .bind_shortcuts(
            &session,
            &[
                NewShortcut::new("decrease", "Decrease monitor brightness")
                    .preferred_trigger("CTRL+XF86MonBrightnessDown"),
                NewShortcut::new("increase", "Increase monitor brightness")
                    .preferred_trigger("CTRL+XF86MonBrightnessUp"),
            ],
            None,
            Default::default(),
        )
        .await?
        .response()?;
    let description = bound
        .shortcuts()
        .iter()
        .map(|s| format!("{}: {}", s.description(), s.trigger_description()))
        .collect::<Vec<_>>()
        .join(" · ");
    let _ = tx.send(Action::ShortcutStatus(true, description)).await;
    let session_path = serde_json::to_value(&*session)
        .expect("session serializes as a path")
        .as_str()
        .expect("session path is a string")
        .to_owned();
    let closed_session = session.clone();
    let task = tokio::spawn(async move {
        let mut closed = match closed_session.receive_closed().await {
            Ok(stream) => stream,
            Err(e) => {
                let _ = tx.send(Action::ShortcutStatus(false, e.to_string())).await;
                return;
            }
        };
        loop {
            tokio::select! {
                event = activated.next() => {
                    let Some(event) = event else { break; };
                    if event.session_handle().as_str() != session_path { continue; }
                    match event.shortcut_id() {
                        "increase" => { let _ = tx.send(Action::ShortcutReceived(1)).await; }
                        "decrease" => { let _ = tx.send(Action::ShortcutReceived(-1)).await; }
                        _ => (),
                    }
                }
                event = changed.next() => {
                    let Some(event) = event else { break; };
                    if event.session_handle().as_str() == session_path {
                        let description = event.shortcuts().iter().map(|s| format!("{}: {}", s.description(), s.trigger_description())).collect::<Vec<_>>().join(" · ");
                        let _ = tx.send(Action::ShortcutStatus(true, description)).await;
                    }
                }
                _ = closed.next() => break,
            }
        }
        let _ = tx
            .send(Action::ShortcutStatus(
                false,
                "Shortcut connection closed. Reconnect in Settings.".into(),
            ))
            .await;
    });
    Ok((portal, session, task))
}
async fn watch_monitors(tx: Sender<Action>) -> zbus::Result<()> {
    let connection = zbus::Connection::session().await?;
    let proxy = zbus::Proxy::new(
        &connection,
        "org.gnome.Mutter.DisplayConfig",
        "/org/gnome/Mutter/DisplayConfig",
        "org.gnome.Mutter.DisplayConfig",
    )
    .await?;
    let mut changed = proxy.receive_signal("MonitorsChanged").await?;
    let properties = zbus::Proxy::new(
        &connection,
        "org.gnome.Mutter.DisplayConfig",
        "/org/gnome/Mutter/DisplayConfig",
        "org.freedesktop.DBus.Properties",
    )
    .await?;
    let mut power = properties.receive_signal("PropertiesChanged").await?;
    loop {
        tokio::select! {
            message = changed.next() => {
                if message.is_none() { break; }
                let _ = tx.send(Action::Topology).await;
            }
            message = power.next() => {
                let Some(message) = message else { break; };
                if let Ok((_, values, _)) = message.body().deserialize::<(String, std::collections::HashMap<String, zbus::zvariant::OwnedValue>, Vec<String>)>() {
                    if values.get("PowerSaveMode").and_then(|v| i32::try_from(v).ok()) == Some(0) {
                        let _ = tx.send(Action::Topology).await;
                    }
                }
            }
        }
    }
    Ok(())
}
async fn watch_resume(tx: Sender<Action>) -> zbus::Result<()> {
    let connection = zbus::Connection::system().await?;
    let proxy = zbus::Proxy::new(
        &connection,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )
    .await?;
    let mut signals = proxy.receive_signal("PrepareForSleep").await?;
    while let Some(message) = signals.next().await {
        if let Ok((sleeping,)) = message.body().deserialize::<(bool,)>() {
            let _ = tx.send(Action::Sleep(sleeping)).await;
        }
    }
    Ok(())
}
fn watch_udev(tx: Sender<Action>) {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    // Kernel multicast socket blocks until a device event; it does not poll.
    let socket = unsafe {
        libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
            libc::NETLINK_KOBJECT_UEVENT,
        )
    };
    if socket < 0 {
        let _ = tx.send_blocking(Action::Notice(format!(
            "Device events unavailable: {}",
            std::io::Error::last_os_error()
        )));
        return;
    }
    let socket = unsafe { OwnedFd::from_raw_fd(socket) };
    let mut address: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    address.nl_family = libc::AF_NETLINK as u16;
    address.nl_groups = 1;
    if unsafe {
        libc::bind(
            socket.as_raw_fd(),
            &address as *const _ as *const libc::sockaddr,
            std::mem::size_of_val(&address) as u32,
        )
    } < 0
    {
        let _ = tx.send_blocking(Action::Notice(format!(
            "Device events unavailable: {}",
            std::io::Error::last_os_error()
        )));
        return;
    }
    let mut buffer = [0u8; 8192];
    loop {
        let count = unsafe {
            libc::recv(
                socket.as_raw_fd(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                0,
            )
        };
        if count < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        let relevant = buffer[..count as usize].split(|b| *b == 0).any(|v| {
            matches!(
                v,
                b"SUBSYSTEM=drm" | b"SUBSYSTEM=i2c" | b"SUBSYSTEM=i2c-dev"
            )
        });
        if relevant && tx.send_blocking(Action::Topology).is_err() {
            break;
        }
    }
}
