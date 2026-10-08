use ashpd::desktop::{
    PersistMode,
    screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType},
};

pub async fn run() -> ashpd::Result<()> {
    let proxy = Screencast::new().await?;
    let session = proxy.create_session(Default::default()).await?;
    proxy
        .select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(CursorMode::Metadata)
                .set_sources(SourceType::Monitor | SourceType::Window)
                .set_multiple(true)
                .set_persist_mode(PersistMode::DoNot),
        )
        .await?;

    let response = proxy
        .start(&session, None, Default::default())
        .await?
        .response()?;
    response.streams().iter().for_each(|stream| {
        println!("node id: {}", stream.pipe_wire_node_id());
        println!("size: {:?}", stream.size());
        println!("position: {:?}", stream.position());
    });
    Ok(())
}