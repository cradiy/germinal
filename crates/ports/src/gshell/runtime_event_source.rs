use std::thread;

pub trait GShellRuntimeEventSource {
    fn spawn(self) -> thread::JoinHandle<()>;
}
