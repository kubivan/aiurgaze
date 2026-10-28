mod protocol;

pub mod game;
pub mod replay;

pub(crate) fn spawn_frame_stream<A>(
    runtime: &bevy_tokio_tasks::TokioTasksRuntime,
    mut responses: tokio::sync::mpsc::Receiver<crate::proxy_channel::TaggedResponse>,
    mut assemble: A,
) where
    A: FnMut(crate::proxy_channel::TaggedResponse) -> Option<crate::frame::Frame>
        + Send
        + 'static,
{
    runtime.spawn_background_task(move |ctx| async move {
        while let Some(response) = responses.recv().await {
            let Some(frame) = assemble(response) else {
                continue;
            };
            let mut ctx_clone = ctx.clone();
            ctx_clone
                .run_on_main_thread(move |ctx| {
                    ctx.world.write_message(crate::frame::FrameEvent { frame });
                })
                .await;
        }
    });
}
