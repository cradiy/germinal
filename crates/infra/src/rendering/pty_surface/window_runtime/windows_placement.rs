use windows_sys::Win32::{
    Foundation::{HWND, POINT, RECT},
    UI::WindowsAndMessaging::*,
};
use winit::{
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::Window,
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SavedWindowPlacement {
    normal: [i32; 4],
    pub maximized: bool,
}

fn hwnd(window: &Window) -> Option<HWND> {
    let RawWindowHandle::Win32(handle) = window.window_handle().ok()?.as_raw() else {
        return None;
    };
    Some(handle.hwnd.get() as HWND)
}

impl SavedWindowPlacement {
    pub fn is_valid(&self) -> bool {
        let [left, top, right, bottom] = self.normal;
        matches!(right.checked_sub(left), Some(100..=32768))
            && matches!(bottom.checked_sub(top), Some(100..=32768))
    }

    pub fn capture(window: &Window) -> Option<Self> {
        let mut placement = WINDOWPLACEMENT {
            length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        if unsafe { GetWindowPlacement(hwnd(window)?, &mut placement) } == 0 {
            return None;
        }
        let rect = placement.rcNormalPosition;
        let state = Self {
            normal: [rect.left, rect.top, rect.right, rect.bottom],
            maximized: placement.showCmd == SW_SHOWMAXIMIZED as u32
                || (placement.showCmd == SW_SHOWMINIMIZED as u32
                    && placement.flags & WPF_RESTORETOMAXIMIZED != 0),
        };
        state.is_valid().then_some(state)
    }

    pub fn restore_hidden(&self, window: &Window) -> bool {
        if !self.is_valid() {
            return false;
        }
        let Some(hwnd) = hwnd(window) else {
            return false;
        };
        let [left, top, right, bottom] = self.normal;
        // Keep workspace coordinates paired with SetWindowPlacement. Windows
        // adjusts offscreen placements when a previously used monitor is gone.
        let placement = WINDOWPLACEMENT {
            length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
            showCmd: SW_HIDE as u32,
            ptMinPosition: POINT { x: -1, y: -1 },
            ptMaxPosition: POINT { x: -1, y: -1 },
            rcNormalPosition: RECT {
                left,
                top,
                right,
                bottom,
            },
            ..Default::default()
        };
        unsafe { SetWindowPlacement(hwnd, &placement) != 0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::{
        application::ApplicationHandler,
        event::WindowEvent,
        event_loop::{ActiveEventLoop, EventLoop},
        platform::windows::EventLoopBuilderExtWindows,
        window::WindowId,
    };

    #[test]
    fn restores_saved_geometry_without_showing_the_window() {
        // winit permits only one event loop per process; isolate this native
        // regression from the existing caption-window regression.
        const CHILD: &str = "GERMINAL_PLACEMENT_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "rendering::pty_surface::window_runtime::windows_placement::tests::restores_saved_geometry_without_showing_the_window", "--nocapture"])
                .env(CHILD, "1").output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        struct Probe;
        impl ApplicationHandler for Probe {
            fn resumed(&mut self, event_loop: &ActiveEventLoop) {
                let window = event_loop
                    .create_window(
                        Window::default_attributes()
                            .with_visible(false)
                            .with_inner_size(winit::dpi::PhysicalSize::new(640, 480)),
                    )
                    .unwrap();
                let original_size = window.inner_size();
                let state = SavedWindowPlacement::capture(&window).unwrap();
                let state: SavedWindowPlacement =
                    serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
                let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(800, 600));
                assert!(state.restore_hidden(&window));
                assert_eq!(window.inner_size(), original_size);
                assert_eq!(unsafe { IsWindowVisible(hwnd(&window).unwrap()) }, 0);
                let invalid = SavedWindowPlacement {
                    normal: [0, 0, -1, 400],
                    maximized: false,
                };
                assert!(!invalid.restore_hidden(&window));
                assert_eq!(window.inner_size(), original_size);
                event_loop.exit();
            }
            fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
        }
        EventLoop::builder()
            .with_any_thread(true)
            .build()
            .unwrap()
            .run_app(&mut Probe)
            .unwrap();
    }
}
