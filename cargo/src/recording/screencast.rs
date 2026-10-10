use ashpd::desktop::{
    PersistMode,
    screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType},
};
use std::sync::mpsc::{Receiver, Sender};

/// Starts the screencast portal and notifies the caller once a source was selected.
pub async fn run(
    ready: Sender<Result<(), String>>,
    shutdown_events: Receiver<()>,
    save_clip_events: Receiver<()>,
) -> Result<(), Box<dyn std::error::Error>> {
    let result = run_inner(&ready, shutdown_events, save_clip_events).await;

    if let Err(error) = &result {
        // The UI is waiting for this result before it creates its window.
        let _ = ready.send(Err(error.to_string()));
    }

    result
}

async fn run_inner(
    ready: &Sender<Result<(), String>>,
    shutdown_events: Receiver<()>,
    save_clip_events: Receiver<()>,
) -> Result<(), Box<dyn std::error::Error>> {
    crate::recording::shortcuts::register_host_app().await?;

    let proxy = Screencast::new().await?;
    let session: ashpd::desktop::Session<Screencast> =
        proxy.create_session(Default::default()).await?;
    proxy
        .select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(CursorMode::Hidden)
                .set_sources(SourceType::Monitor | SourceType::Window)
                .set_multiple(true)
                .set_persist_mode(PersistMode::DoNot),
        )
        .await?;

    let response: ashpd::desktop::screencast::Streams = proxy
        .start(&session, None, Default::default())
        .await?
        .response()?;

    let stream = response
        .streams()
        .first()
        .ok_or("No screencast source was selected")?;

    let node_id = stream.pipe_wire_node_id();

    println!("node id: {node_id}");
    println!("size: {:?}", stream.size());
    println!("position: {:?}", stream.position());

    // The portal has finished its screen-selection flow, so the main thread can
    // now create the application's window while recording setup continues here.
    let _ = ready.send(Ok(()));

    let fd = proxy
        .open_pipe_wire_remote(&session, Default::default())
        .await?;

    println!("PipeWire FD: {fd:?}");

    let shortcut_registration = crate::recording::shortcuts::register().await?;
    crate::recording::pipewire::connect(
        fd,
        node_id,
        shortcut_registration.events,
        shutdown_events,
        save_clip_events,
    )
    .map_err(|error| std::io::Error::other(error.to_string()))?;

    Ok(())
}
