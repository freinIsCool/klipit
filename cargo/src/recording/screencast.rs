use ashpd::desktop::{
    PersistMode,
    screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType},
};

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
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

    let stream = response.streams().first().expect("No streams returned");

    let node_id = stream.pipe_wire_node_id();

    println!("node id: {node_id}");
    println!("size: {:?}", stream.size());
    println!("position: {:?}", stream.position());

    let fd = proxy
        .open_pipe_wire_remote(&session, Default::default())
        .await?;

    println!("PipeWire FD: {fd:?}");

    let shortcut_registration = crate::recording::shortcuts::register().await?;
    crate::recording::pipewire::connect(fd, node_id, shortcut_registration.events)
        .map_err(|error| std::io::Error::other(error.to_string()))?;

    Ok(())
}
