use std::{
    cell::RefCell,
    io,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use pipewire::{self as pw, properties::properties, spa};
use spa::{
    param::{
        ParamType,
        audio::{AudioFormat, AudioInfoRaw},
    },
    pod::{Pod, Value, serialize::PodSerializer},
    utils::{Direction, SpaTypes},
};

use crate::recording::ffmpeg::RecorderCommand;

pub(crate) const SAMPLE_RATE: u32 = 48_000;
pub(crate) const CHANNELS: u32 = 2;

pub(crate) struct AudioCapture {
    stop_requested: Arc<AtomicBool>,
    worker: Option<JoinHandle<Result<(), String>>>,
}

impl AudioCapture {
    pub(crate) fn start(sender: SyncSender<RecorderCommand>) -> io::Result<Self> {
        let stop_requested = Arc::new(AtomicBool::new(false));
        let worker_stop_requested = Arc::clone(&stop_requested);
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("pipewire-audio-capture".into())
            .spawn(move || {
                let result = run_audio_capture(sender, worker_stop_requested, ready_sender.clone());
                if let Err(error) = &result {
                    let _ = ready_sender.try_send(Err(error.clone()));
                }
                result
            })?;

        match ready_receiver.recv() {
            Ok(Ok(())) => Ok(Self {
                stop_requested,
                worker: Some(worker),
            }),
            Ok(Err(error)) => {
                let _ = worker.join();
                Err(io::Error::other(error))
            }
            Err(error) => {
                let _ = worker.join();
                Err(io::Error::other(format!(
                    "Audio capture stopped before it connected: {error}"
                )))
            }
        }
    }

    pub(crate) fn stop(mut self) -> Result<(), String> {
        self.stop_requested.store(true, Ordering::Release);
        self.worker
            .take()
            .expect("audio capture worker is present")
            .join()
            .map_err(|_| "Audio capture thread panicked".to_owned())?
    }
}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        self.stop_requested.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                eprintln!("Audio capture thread panicked while stopping.");
            }
        }
    }
}

fn run_audio_capture(
    sender: SyncSender<RecorderCommand>,
    stop_requested: Arc<AtomicBool>,
    ready_sender: SyncSender<Result<(), String>>,
) -> Result<(), String> {
    let mainloop = Rc::new(pw::main_loop::MainLoopBox::new(None).map_err(|e| e.to_string())?);
    let context =
        pw::context::ContextBox::new(&mainloop.loop_(), None).map_err(|e| e.to_string())?;
    let core = context.connect(None).map_err(|e| e.to_string())?;

    let stream = pw::stream::StreamBox::new(
        &core,
        "record_desktop_audio",
        properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Music",
            *pw::keys::STREAM_CAPTURE_SINK => "true",
        },
    )
    .map_err(|e| e.to_string())?;

    let stream_error = Rc::new(RefCell::new(None));
    let state_mainloop = Rc::clone(&mainloop);
    let state_error = Rc::clone(&stream_error);
    let _listener = stream
        .add_local_listener_with_user_data(())
        .state_changed(move |_, _, old, new| match new {
            pw::stream::StreamState::Error(error) => {
                *state_error.borrow_mut() = Some(error.clone());
                state_mainloop.quit();
            }
            pw::stream::StreamState::Unconnected if old != pw::stream::StreamState::Unconnected => {
                *state_error.borrow_mut() =
                    Some("PipeWire audio stream disconnected unexpectedly".to_owned());
                state_mainloop.quit();
            }
            _ => {}
        })
        .process({
            let process_mainloop = Rc::clone(&mainloop);
            let process_error = Rc::clone(&stream_error);
            move |stream, _| {
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };

                let Some(data) = buffer.datas_mut().first_mut() else {
                    return;
                };
                let offset = data.chunk().offset() as usize;
                let size = data.chunk().size() as usize;
                let Some(bytes) = data.data() else {
                    return;
                };
                let Some(end) = offset.checked_add(size).filter(|end| *end <= bytes.len()) else {
                    *process_error.borrow_mut() =
                        Some("PipeWire returned an invalid audio buffer range".to_owned());
                    process_mainloop.quit();
                    return;
                };

                if size == 0 {
                    return;
                }
                if let Err(error) = sender.send(RecorderCommand::Audio(bytes[offset..end].to_vec()))
                {
                    *process_error.borrow_mut() =
                        Some(format!("Audio recorder stopped unexpectedly: {error}"));
                    process_mainloop.quit();
                }
            }
        })
        .register()
        .map_err(|e| e.to_string())?;

    let mut audio_info = AudioInfoRaw::new();
    audio_info.set_format(AudioFormat::S16LE);
    audio_info.set_rate(SAMPLE_RATE);
    audio_info.set_channels(CHANNELS);
    let object = Value::Object(spa::pod::Object {
        type_: SpaTypes::ObjectParamFormat.as_raw(),
        id: ParamType::EnumFormat.as_raw(),
        properties: audio_info.into(),
    });
    let bytes = PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &object)
        .map_err(|e| e.to_string())?
        .0
        .into_inner();
    let pod = Pod::from_bytes(&bytes).ok_or("Failed to create audio SPA Pod")?;
    let mut params = [&*pod];

    stream
        .connect(
            Direction::Input,
            None,
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(|e| e.to_string())?;

    let timer_mainloop = Rc::clone(&mainloop);
    let timer_stop_requested = Arc::clone(&stop_requested);
    let timer = mainloop.loop_().add_timer(move |_| {
        if timer_stop_requested.load(Ordering::Acquire) {
            timer_mainloop.quit();
        }
    });
    timer
        .update_timer(
            Some(Duration::from_millis(50)),
            Some(Duration::from_millis(50)),
        )
        .into_result()
        .map_err(|e| e.to_string())?;
    ready_sender
        .send(Ok(()))
        .map_err(|e| format!("Failed to report audio capture startup: {e}"))?;

    mainloop.run();
    drop(_listener);
    drop(timer);

    match stream_error.borrow_mut().take() {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
