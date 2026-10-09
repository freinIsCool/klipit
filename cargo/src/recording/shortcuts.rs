use std::{
    env, fs, io,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
};

use ashpd::desktop::{
    Session,
    global_shortcuts::{Activated, GlobalShortcuts, NewShortcut},
};
use futures_util::StreamExt;
use notify_rust::Notification;

const APP_ID: &str = "io.github.freinisCool.Klipit";

pub struct Registration {
    pub events: Receiver<()>,
    _session: Session<GlobalShortcuts>,
}

pub fn ensure_desktop_entry() -> io::Result<()> {
    let data_home = match env::var_os("XDG_DATA_HOME").map(PathBuf::from) {
        Some(path) if path.is_absolute() => path,
        _ => env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?
            .join(".local/share"),
    };
    let applications = data_home.join("applications");
    fs::create_dir_all(&applications)?;

    let desktop_file = applications.join(format!("{APP_ID}.desktop"));
    if desktop_file.exists() {
        return Ok(());
    }

    let executable = env::current_exe()?;
    let executable = executable.to_str().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "Executable path is not UTF-8")
    })?;
    let executable = executable
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
        .replace('`', "\\`");
    let desktop_entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Klipit\n\
         Exec=\"{executable}\"\n\
         Terminal=false\n\
         Categories=AudioVideo;Video;\n\
         StartupNotify=false\n"
    );
    fs::write(desktop_file, desktop_entry)
}

pub async fn register_host_app() -> ashpd::Result<()> {
    ensure_desktop_entry()?;
    ashpd::register_host_app(ashpd::AppID::try_from(APP_ID)?).await
}

pub async fn register() -> ashpd::Result<Registration> {
    let portal = GlobalShortcuts::new().await?;
    let session = portal.create_session(Default::default()).await?;
    let mut activated = portal.receive_activated().await?;
    let shortcut =
        NewShortcut::new("save-clip", "Save the recent video clip").preferred_trigger("CTRL+ALT+s");

    let bound = portal
        .bind_shortcuts(&session, &[shortcut], None, Default::default())
        .await?
        .response()?;
    for shortcut in bound.shortcuts() {
        println!(
            "Global shortcut {} is bound to {}",
            shortcut.id(),
            shortcut.trigger_description()
        );
    }

    let (sender, receiver) = mpsc::channel();
    tokio::spawn(async move {
        forward_activations(&mut activated, sender).await;
    });

    Ok(Registration {
        events: receiver,
        _session: session,
    })
}

async fn forward_activations(
    activated: &mut (impl futures_util::Stream<Item = Activated> + Unpin),
    sender: Sender<()>,
) {
    while let Some(event) = activated.next().await {
        if event.shortcut_id() == "save-clip" {
            if let Err(error) = Notification::new()
                .summary("Clip Saved!")
                .body("clip has been Saved")
                .show_async()
                .await
            {
                eprintln!("Failed to send clip notification: {error}");
            }
            println!("Save clip shortcut activated");
            if sender.send(()).is_err() {
                break;
            }
        }
    }
}
