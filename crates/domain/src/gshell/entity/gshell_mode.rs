#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GShellMode {
	Pty,
	GNativeConnecting,
	GNative,
}
