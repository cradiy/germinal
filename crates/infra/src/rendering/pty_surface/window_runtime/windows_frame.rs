//! Native non-client hit testing for the custom Windows toolbar.
use std::{
    cell::{Cell, RefCell},
    sync::Arc,
};

use germinal_ports::{
    pty_host::scale_factor::TerminalScaleFactor,
    rendering::toolbar::{
        ToolbarAction, ToolbarConfig, toolbar_buttons, toolbar_height, toolbar_hit,
    },
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::{
        Dwm::{DwmDefWindowProc, DwmExtendFrameIntoClientArea},
        Gdi::{InvalidateRect, ScreenToClient},
    },
    UI::{
        Controls::MARGINS,
        HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi},
        Input::KeyboardAndMouse::{TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent},
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::*,
    },
};
use winit::{
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::Window,
};

const SUBCLASS_ID: usize = 0x4745524d;

pub(super) fn cloak_for_startup(window: &Window, cloaked: bool) {
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAK, DwmSetWindowAttribute};
    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    let value = i32::from(cloaked);
    let result = unsafe {
        DwmSetWindowAttribute(
            handle.hwnd.get() as HWND,
            DWMWA_CLOAK as u32,
            (&value as *const i32).cast(),
            std::mem::size_of_val(&value) as u32,
        )
    };
    if result < 0 {
        tracing::warn!(result, "failed to update startup window cloak");
    }
}

#[derive(Default)]
struct FrameState {
    config: Cell<Option<FrameConfig>>,
    hover: Cell<Option<ToolbarAction>>,
    hover_changed: Cell<bool>,
    pressed_caption: Cell<Option<u32>>,
    tab_regions: RefCell<Vec<RECT>>,
}

impl FrameState {
    fn set_hover(&self, hwnd: HWND, hover: Option<ToolbarAction>) {
        if self.hover.replace(hover) != hover {
            self.hover_changed.set(true);
            // Redraw without synthesizing client mouse messages. Client mouse tracking
            // would compete with Windows' non-client hover timer for Snap Layouts.
            unsafe {
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
        }
    }
}

#[derive(Clone, Copy)]
struct FrameConfig {
    toolbar: ToolbarConfig,
    sidebar_width: u32,
}

pub(super) struct WindowsFrame {
    // Keep the HWND alive until the subclass has been removed.
    _window: Arc<Window>,
    hwnd: HWND,
    state: Box<FrameState>,
}

impl WindowsFrame {
    pub(super) fn update_tab_regions(&self, surface: Option<&super::WgpuTabBarSurface>) {
        let mut regions = self.state.tab_regions.borrow_mut();
        regions.clear();
        if let Some(surface) = surface {
            regions.extend(surface.hit_regions.iter().map(|region| {
                RECT {
                    left: (surface.placement.x_px + region.start_column * surface.cell_width_px)
                        as i32,
                    right: (surface.placement.x_px + region.end_column * surface.cell_width_px)
                        .min(surface.placement.x_px + surface.placement.width_px)
                        as i32,
                    top: (surface.placement.y_px + region.top_px) as i32,
                    bottom: (surface.placement.y_px + region.bottom_px)
                        .min(surface.placement.y_px + surface.placement.height_px)
                        as i32,
                }
            }));
        }
    }

    pub(super) fn install(window: Arc<Window>) -> Option<Self> {
        let RawWindowHandle::Win32(handle) = window.window_handle().ok()?.as_raw() else {
            return None;
        };
        let hwnd = handle.hwnd.get() as HWND;
        let state = Box::<FrameState>::default();
        // Installed and updated on winit's window thread. The boxed state has a stable
        // address, and Drop removes the callback before freeing it.
        if unsafe {
            SetWindowSubclass(
                hwnd,
                Some(frame_proc),
                SUBCLASS_ID,
                state.as_ref() as *const _ as usize,
            )
        } == 0
        {
            tracing::warn!("failed to install Windows toolbar hit testing");
            return None;
        }
        Some(Self {
            _window: window,
            hwnd,
            state,
        })
    }

    pub(super) fn update(&self, toolbar: Option<ToolbarConfig>, sidebar_width: u32) {
        let was_custom = self.state.config.get().is_some();
        self.state.config.set(toolbar.map(|toolbar| FrameConfig {
            toolbar,
            sidebar_width,
        }));
        if was_custom != toolbar.is_some() {
            update_dwm_frame(self.hwnd, toolbar.is_some());
        }
    }

    pub(super) fn take_hover_change(&self) -> Option<Option<ToolbarAction>> {
        self.state
            .hover_changed
            .replace(false)
            .then(|| self.state.hover.get())
    }
}

impl Drop for WindowsFrame {
    fn drop(&mut self) {
        // The retained Window keeps hwnd valid; this runs on its owning thread.
        unsafe {
            RemoveWindowSubclass(self.hwnd, Some(frame_proc), SUBCLASS_ID);
        }
    }
}

fn update_dwm_frame(hwnd: HWND, custom: bool) {
    // Keep the top client edge flush with the window. Extending the left edge
    // preserves the DWM shadow without a top frame competing with custom captions.
    // See VLC mainctx_win32.cpp, updateClientFrame (commit cdb94e5e).
    let margins = MARGINS {
        cxLeftWidth: i32::from(custom),
        ..Default::default()
    };
    let result = unsafe { DwmExtendFrameIntoClientArea(hwnd, &margins) };
    if result < 0 {
        tracing::warn!(result, "failed to update custom Windows frame");
    }
}

unsafe extern "system" fn frame_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    // Copy the state before calling Windows: native message handling can reenter us.
    let state = unsafe { &*(data as *const FrameState) };
    let config = state.config.get();
    if let Some(config) = config {
        match message {
            WM_DWMCOMPOSITIONCHANGED | WM_SHOWWINDOW => {
                update_dwm_frame(hwnd, true);
            }
            WM_NCHITTEST => {
                let mut point = screen_point(lparam);
                let mut rect = RECT::default();
                if unsafe { ScreenToClient(hwnd, &mut point) } != 0
                    && unsafe { GetClientRect(hwnd, &mut rect) } != 0
                {
                    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
                    let scale = TerminalScaleFactor::new(f64::from(dpi) / 96.0);
                    let border = unsafe {
                        GetSystemMetricsForDpi(SM_CXSIZEFRAME, dpi)
                            + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi)
                    };
                    let resizable =
                        unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32 & WS_THICKFRAME != 0;
                    let maximized = unsafe { IsZoomed(hwnd) } != 0;
                    let hit = hit_test(
                        config,
                        rect.right,
                        rect.bottom,
                        point,
                        scale,
                        if resizable && !maximized { border } else { 0 },
                    );
                    // Only rendered tabs consume caption input. Unused space in
                    // the inline viewport is a real native caption, including
                    // double-click maximize/restore and the system menu.
                    if hit == HTCLIENT
                        && config.toolbar.inline_tabs
                        && point.y < toolbar_height(scale, rect.bottom as u32) as i32
                        && !self::point_in_tab_regions(point, &state.tab_regions.borrow())
                    {
                        return HTCAPTION as LRESULT;
                    }
                    return hit as LRESULT;
                }
            }
            WM_NCMOUSEMOVE => {
                state.set_hover(
                    hwnd,
                    match wparam as u32 {
                        HTMINBUTTON => Some(ToolbarAction::Minimize),
                        HTMAXBUTTON => Some(ToolbarAction::Maximize),
                        HTCLOSE => Some(ToolbarAction::Close),
                        _ => None,
                    },
                );
                let mut tracking = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    // Leave the system caption-hover timer to DefWindowProc.
                    dwFlags: TME_LEAVE | TME_NONCLIENT,
                    hwndTrack: hwnd,
                    dwHoverTime: u32::MAX, // HOVER_DEFAULT
                };
                unsafe {
                    TrackMouseEvent(&mut tracking);
                }
                // Snap Layouts is initiated by DefWindowProc's caption tracking.
                // DwmDefWindowProc can consume this message without running that
                // tracking, so do not short-circuit it through DWM below.
                return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
            }
            WM_NCMOUSEHOVER => {
                return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
            }
            WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK
                if matches!(wparam as u32, HTMINBUTTON | HTMAXBUTTON | HTCLOSE) =>
            {
                state.pressed_caption.set(Some(wparam as u32));
                return 0;
            }
            WM_NCLBUTTONUP => {
                if let Some(pressed) = state.pressed_caption.take() {
                    if pressed == wparam as u32 {
                        let command = match pressed {
                            HTMINBUTTON => SC_MINIMIZE,
                            HTMAXBUTTON if unsafe { IsZoomed(hwnd) } != 0 => SC_RESTORE,
                            HTMAXBUTTON => SC_MAXIMIZE,
                            _ => SC_CLOSE,
                        };
                        // The native caption tracker uses system button rectangles,
                        // which differ from our custom layout. Dispatch its command
                        // only after a matching press/release in our hit regions.
                        unsafe {
                            PostMessageW(hwnd, WM_SYSCOMMAND, command as WPARAM, 0);
                        }
                    }
                    return 0;
                }
            }
            WM_NCMOUSELEAVE => {
                state.pressed_caption.set(None);
                state.set_hover(hwnd, None);
            }
            _ => {}
        }
        // DWM must see non-client hover/leave messages for native caption behavior.
        // Keep WM_NCHITTEST above tied to our rendered button geometry.
        if matches!(
            message,
            WM_NCMOUSELEAVE | WM_NCLBUTTONDOWN | WM_NCLBUTTONUP | WM_NCLBUTTONDBLCLK
        ) {
            let mut result = 0;
            if unsafe { DwmDefWindowProc(hwnd, message, wparam, lparam, &mut result) } != 0 {
                return result;
            }
        }
    }
    // DefWindowProc (through winit) implements drag/restore, double-click maximize,
    // the system menu, resize cursors, and the Windows 11 maximize-button flyout.
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

fn point_in_tab_regions(point: POINT, regions: &[RECT]) -> bool {
    regions.iter().any(|rect| {
        point.x >= rect.left && point.x < rect.right && point.y >= rect.top && point.y < rect.bottom
    })
}

fn screen_point(lparam: LPARAM) -> POINT {
    // Screen coordinates are signed, including monitors left/above the primary one.
    POINT {
        x: lparam as i16 as i32,
        y: (lparam >> 16) as i16 as i32,
    }
}

fn hit_test(
    config: FrameConfig,
    width: i32,
    height: i32,
    point: POINT,
    scale: TerminalScaleFactor,
    border: i32,
) -> u32 {
    let POINT { x, y } = point;
    if x < 0 || y < 0 || x >= width || y >= height {
        return HTNOWHERE;
    }
    if border > 0 {
        let left = x < border;
        let right = x >= width - border;
        let top = y < border;
        let bottom = y >= height - border;
        match (left, right, top, bottom) {
            (true, _, true, _) => return HTTOPLEFT,
            (_, true, true, _) => return HTTOPRIGHT,
            (true, _, _, true) => return HTBOTTOMLEFT,
            (_, true, _, true) => return HTBOTTOMRIGHT,
            (true, _, _, _) => return HTLEFT,
            (_, true, _, _) => return HTRIGHT,
            (_, _, true, _) => return HTTOP,
            (_, _, _, true) => return HTBOTTOM,
            _ => {}
        }
    }
    let toolbar_height = toolbar_height(scale, height as u32);
    if y >= toolbar_height as i32 {
        return HTCLIENT;
    }
    let buttons = toolbar_buttons(
        config.toolbar,
        width as u32,
        toolbar_height,
        config.sidebar_width,
        scale,
    );
    match toolbar_hit(&buttons, toolbar_height, f64::from(x), f64::from(y)) {
        Some(ToolbarAction::Minimize) => HTMINBUTTON,
        Some(ToolbarAction::Maximize) => HTMAXBUTTON,
        Some(ToolbarAction::Close) => HTCLOSE,
        Some(_) => HTCLIENT,
        None => {
            let (left, top, tab_width, tab_height) =
                germinal_ports::rendering::toolbar::inline_tab_bounds(
                    config.toolbar,
                    width as u32,
                    toolbar_height,
                    scale,
                );
            if config.toolbar.inline_tabs
                && x >= left as i32
                && x < (left + tab_width) as i32
                && y >= top as i32
                && y < (top + tab_height) as i32
            {
                HTCLIENT
            } else {
                HTCAPTION
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_window_routes_native_caption_hits_and_detaches_cleanly() {
        use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
        use winit::{
            application::ApplicationHandler,
            event::WindowEvent,
            event_loop::{ActiveEventLoop, EventLoop},
            platform::windows::{EventLoopBuilderExtWindows, WindowAttributesExtWindows},
            window::WindowId,
        };

        struct Probe;
        impl ApplicationHandler for Probe {
            fn resumed(&mut self, event_loop: &ActiveEventLoop) {
                let window = Arc::new(
                    event_loop
                        .create_window(
                            Window::default_attributes()
                                .with_visible(false)
                                .with_decorations(false)
                                .with_undecorated_shadow(false)
                                .with_inner_size(winit::dpi::PhysicalSize::new(900, 500)),
                        )
                        .expect("create hidden native window"),
                );
                let frame = WindowsFrame::install(window).expect("install subclass");
                let hwnd = frame.hwnd;
                let send_hit = |x, y| {
                    let mut point = POINT { x, y };
                    // The OS supplies screen coordinates to WM_NCHITTEST.
                    assert_ne!(unsafe { ClientToScreen(hwnd, &mut point) }, 0);
                    let packed =
                        (point.x as u16 as u32 | ((point.y as u16 as u32) << 16)) as LPARAM;
                    unsafe { SendMessageW(hwnd, WM_NCHITTEST, 0, packed) }
                };
                let normal_hit = send_hit(450, 20);
                frame.update(
                    Some(ToolbarConfig {
                        modern: false,
                        inline_tabs: false,
                        sidebar: false,
                        window_controls: true,
                    }),
                    0,
                );
                // Keep the custom caption flush with the window's top edge; the
                // old winit shadow path shifted it down by one native pixel.
                let mut origin = POINT::default();
                let mut outer = RECT::default();
                assert_ne!(unsafe { ClientToScreen(hwnd, &mut origin) }, 0);
                assert_ne!(unsafe { GetWindowRect(hwnd, &mut outer) }, 0);
                assert_eq!(origin.y, outer.top);
                assert_eq!(send_hit(450, 20), HTCAPTION as isize);
                let mut rect = RECT::default();
                assert_ne!(unsafe { GetClientRect(hwnd, &mut rect) }, 0);
                let scale =
                    TerminalScaleFactor::new(f64::from(unsafe { GetDpiForWindow(hwnd) }) / 96.0);
                let h = toolbar_height(scale, rect.bottom as u32) as i32;
                assert_eq!(
                    send_hit(rect.right - h * 3 / 2, h / 2),
                    HTMAXBUTTON as isize
                );
                assert_eq!(send_hit(h / 2, h / 2), HTCLIENT as isize);
                assert_eq!(send_hit(1, 1), HTTOPLEFT as isize);
                // Non-client hover must not enqueue synthetic client mouse movement:
                // winit would then start a competing client-area mouse tracker.
                let mut message = MSG::default();
                while unsafe {
                    PeekMessageW(&mut message, hwnd, WM_MOUSEMOVE, WM_MOUSEMOVE, PM_REMOVE)
                } != 0
                {}
                let mut point = POINT {
                    x: rect.right - h * 3 / 2,
                    y: h / 2,
                };
                assert_ne!(unsafe { ClientToScreen(hwnd, &mut point) }, 0);
                let packed = (point.x as u16 as u32 | ((point.y as u16 as u32) << 16)) as LPARAM;
                unsafe {
                    SendMessageW(hwnd, WM_NCMOUSEMOVE, HTMAXBUTTON as WPARAM, packed);
                }
                assert_eq!(
                    frame.take_hover_change(),
                    Some(Some(ToolbarAction::Maximize))
                );
                assert_eq!(
                    unsafe {
                        PeekMessageW(&mut message, hwnd, WM_MOUSEMOVE, WM_MOUSEMOVE, PM_REMOVE)
                    },
                    0
                );
                unsafe {
                    SendMessageW(hwnd, WM_NCMOUSELEAVE, 0, 0);
                }
                assert_eq!(frame.take_hover_change(), Some(None));
                frame.update(
                    Some(ToolbarConfig {
                        modern: false,
                        inline_tabs: true,
                        sidebar: false,
                        window_controls: true,
                    }),
                    0,
                );
                frame.state.tab_regions.borrow_mut().push(RECT {
                    left: h,
                    right: h * 3,
                    top: h / 4,
                    bottom: h * 3 / 4,
                });
                assert_eq!(send_hit(h * 2, h / 2), HTCLIENT as isize);
                assert_eq!(send_hit(h * 4, h / 2), HTCAPTION as isize);
                frame.update_tab_regions(None);
                assert_eq!(send_hit(h * 2, h / 2), HTCAPTION as isize);
                frame.update(None, 0);
                assert_eq!(send_hit(450, 20), normal_hit);
                // Keep the HWND alive while proving Drop removed the callback and its state.
                let window = Arc::clone(&frame._window);
                drop(frame);
                assert_eq!(send_hit(450, 20), normal_hit);
                drop(window);
                event_loop.exit();
            }

            fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
        }

        let event_loop = EventLoop::builder()
            .with_any_thread(true)
            .build()
            .expect("create Windows event loop");
        event_loop
            .run_app(&mut Probe)
            .expect("run hidden window probe");
    }

    #[test]
    fn native_hit_regions_follow_drawn_controls_and_preserve_resize_edges() {
        let config = FrameConfig {
            toolbar: ToolbarConfig {
                modern: false,
                inline_tabs: false,
                sidebar: false,
                window_controls: true,
            },
            sidebar_width: 0,
        };
        for factor in [1.0, 1.5, 2.0] {
            let scale = TerminalScaleFactor::new(factor);
            let at = |x, y, border| hit_test(config, 1200, 800, POINT { x, y }, scale, border);
            let h = toolbar_height(scale, 800) as i32;
            assert_eq!(at(1200 - h * 3 / 2, h / 2, 8), HTMAXBUTTON);
            assert_eq!(at(h / 2, h / 2, 8), HTCLIENT);
            assert_eq!(at(500, h / 2, 8), HTCAPTION);
            assert_eq!(at(500, h, 8), HTCLIENT);
            assert_eq!(at(1, 1, 8), HTTOPLEFT);
            assert_eq!(at(1199, 799, 8), HTBOTTOMRIGHT);
            // Maximized windows have no resize band, even at the top screen edge.
            assert_eq!(at(500, 0, 0), HTCAPTION);
            assert_eq!(at(1200 - h * 3 / 2, 0, 0), HTMAXBUTTON);
            let inline = FrameConfig {
                toolbar: ToolbarConfig {
                    modern: false,
                    inline_tabs: true,
                    ..config.toolbar
                },
                ..config
            };
            assert_eq!(
                hit_test(inline, 1200, 800, POINT { x: 100, y: h / 2 }, scale, 8),
                HTCLIENT
            );
            assert_eq!(
                hit_test(
                    inline,
                    1200,
                    800,
                    POINT {
                        x: 1200 - h * 3 - 12,
                        y: h / 2
                    },
                    scale,
                    8
                ),
                HTCAPTION
            );
            assert_eq!(
                hit_test(
                    inline,
                    1200,
                    800,
                    POINT {
                        x: 1200 - h * 3 / 2,
                        y: h / 2
                    },
                    scale,
                    8
                ),
                HTMAXBUTTON
            );
        }
        let hidden_controls = FrameConfig {
            toolbar: ToolbarConfig {
                window_controls: false,
                ..config.toolbar
            },
            ..config
        };
        assert_eq!(
            hit_test(
                hidden_controls,
                1200,
                800,
                POINT { x: 1140, y: 20 },
                TerminalScaleFactor::DEFAULT,
                0
            ),
            HTCAPTION
        );
        let point =
            screen_point(((-200_i16 as u16 as u32) << 16 | (-100_i16 as u16 as u32)) as LPARAM);
        assert_eq!((point.x, point.y), (-100, -200));
    }
}
