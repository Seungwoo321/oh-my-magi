use tauri::{
    AppHandle, Emitter, LogicalPosition, Manager, PhysicalPosition, RunEvent, WindowEvent, Wry,
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

use crate::commands::{ExitAuthorization, focus_main_window};

const POPOVER_WIDTH: f64 = 440.0;
const POPOVER_HEIGHT: f64 = 560.0;

pub(crate) fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .menu(build_app_menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "magi-console-open" => {
                let _ = focus_main_window(app);
            }
            "magi-settings" => {
                if focus_main_window(app).is_ok() {
                    let _ = app.emit_to("main", "magi:open-settings", ());
                }
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            crate::commands::get_console_snapshot,
            crate::commands::select_context_files,
            crate::commands::select_context_directory,
            crate::preferences::get_console_preferences,
            crate::preferences::save_console_preferences,
            crate::commands::shell_context,
            crate::commands::shell_open_console,
            crate::commands::shell_open_settings,
            crate::commands::shell_close_companion,
            crate::commands::shell_request_exit,
            crate::commands::shell_confirm_exit,
            crate::profiles::list_recent_runs,
            crate::profiles::list_acp_adapters,
            crate::profiles::list_provider_profiles,
            crate::profiles::load_active_provider_profile_selection,
            crate::profiles::save_provider_profile,
            crate::profiles::set_active_provider_profile,
            crate::profiles::validate_provider_profile,
            crate::profiles::list_role_presets,
            crate::profiles::load_active_role_preset_selection,
            crate::profiles::set_active_role_preset,
            crate::profiles::clone_role_preset,
            crate::profiles::save_role_preset
        ])
        .setup(|app| {
            app.manage(ExitAuthorization::new());
            app.manage(crate::commands::DesktopState::open(app.handle()));
            install_tray(app.handle())?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to initialize MAGI CONSOLE");

    app.run(handle_run_event);
}

fn build_app_menu(app: &AppHandle<Wry>) -> tauri::Result<Menu<Wry>> {
    let app_menu = Submenu::with_items(
        app,
        "MAGI CONSOLE",
        true,
        &[
            &PredefinedMenuItem::about(app, None, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;
    let show_console =
        MenuItem::with_id(app, "magi-console-open", "콘솔 열기", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "magi-settings", "설정…", true, None::<&str>)?;
    let file_menu = Submenu::with_items(
        app,
        "파일",
        true,
        &[
            &show_console,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;
    let edit_menu = Submenu::with_items(
        app,
        "편집",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;

    Menu::with_items(app, &[&app_menu, &file_menu, &edit_menu])
}

fn install_tray(app: &AppHandle<Wry>) -> Result<(), Box<dyn std::error::Error>> {
    let open_console = MenuItem::with_id(app, "open-console", "콘솔 열기", true, None::<&str>)?;
    let open_settings = MenuItem::with_id(app, "open-settings", "설정", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "앱 종료…", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open_console, &open_settings, &quit])?;
    let tray = TrayIconBuilder::with_id("magi-status")
        .icon(tray_image())
        .icon_as_template(true)
        .tooltip("MAGI CONSOLE")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open-console" => {
                let _ = focus_main_window(app);
            }
            "open-settings" => {
                if focus_main_window(app).is_ok() {
                    let _ = app.emit_to("main", "magi:open-settings", ());
                }
            }
            "quit" => request_exit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                position,
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_companion(tray.app_handle(), position);
            }
        })
        .build(app)?;

    app.manage(tray);
    Ok(())
}

fn handle_run_event(app: &AppHandle<Wry>, event: RunEvent) {
    match event {
        RunEvent::WindowEvent { label, event, .. } => match event {
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                if let Some(window) = app.get_webview_window(&label) {
                    let _ = window.hide();
                }
            }
            WindowEvent::Focused(false) if label == "companion" => {
                if let Some(window) = app.get_webview_window("companion") {
                    let _ = window.hide();
                }
            }
            _ => {}
        },
        RunEvent::ExitRequested { api, .. } => {
            if !app.state::<ExitAuthorization>().is_authorized() {
                api.prevent_exit();
                request_exit(app);
            }
        }
        _ => {}
    }
}

fn request_exit(app: &AppHandle<Wry>) {
    if focus_main_window(app).is_ok() {
        let _ = app.emit_to("main", "magi:exit-requested", ());
    }
}

fn toggle_companion(app: &AppHandle<Wry>, anchor: PhysicalPosition<f64>) {
    let Some(companion) = app.get_webview_window("companion") else {
        return;
    };
    if companion.is_visible().unwrap_or(false) {
        let _ = companion.hide();
        return;
    }
    if let Some(position) = popover_position(app, anchor) {
        let _ = companion.set_position(position);
    } else {
        let _ = companion.center();
    }
    if companion.show().is_ok() {
        let _ = companion.set_focus();
    }
}

fn popover_position(
    app: &AppHandle<Wry>,
    anchor: PhysicalPosition<f64>,
) -> Option<LogicalPosition<f64>> {
    let monitor = app
        .monitor_from_point(anchor.x, anchor.y)
        .ok()
        .flatten()
        .or_else(|| app.primary_monitor().ok().flatten())?;
    let scale = monitor.scale_factor();
    let work = monitor.work_area();
    let left = f64::from(work.position.x) / scale;
    let top = f64::from(work.position.y) / scale;
    let right = left + f64::from(work.size.width) / scale;
    let bottom = top + f64::from(work.size.height) / scale;
    let max_x = (right - POPOVER_WIDTH).max(left);
    let max_y = (bottom - POPOVER_HEIGHT).max(top);
    let x = (anchor.x / scale - POPOVER_WIDTH / 2.0).clamp(left, max_x);
    let y = (anchor.y / scale + 10.0).clamp(top, max_y);
    Some(LogicalPosition::new(x, y))
}

fn tray_image() -> Image<'static> {
    const SIZE: i32 = 32;
    let mut pixels = vec![0; (SIZE * SIZE * 4) as usize];
    let color = [248, 235, 220, 255];
    draw_line(&mut pixels, SIZE, (16, 3), (29, 27), color);
    draw_line(&mut pixels, SIZE, (29, 27), (3, 27), color);
    draw_line(&mut pixels, SIZE, (3, 27), (16, 3), color);
    draw_line(&mut pixels, SIZE, (16, 12), (8, 23), color);
    draw_line(&mut pixels, SIZE, (16, 12), (24, 23), color);
    draw_line(&mut pixels, SIZE, (8, 23), (24, 23), color);
    Image::new_owned(pixels, SIZE as u32, SIZE as u32)
}

fn draw_line(pixels: &mut [u8], size: i32, start: (i32, i32), end: (i32, i32), color: [u8; 4]) {
    let (mut x, mut y) = start;
    let dx = (end.0 - x).abs();
    let sx = if x < end.0 { 1 } else { -1 };
    let dy = -(end.1 - y).abs();
    let sy = if y < end.1 { 1 } else { -1 };
    let mut error = dx + dy;

    loop {
        set_pixel(pixels, size, x, y, color);
        set_pixel(pixels, size, x + 1, y, color);
        set_pixel(pixels, size, x, y + 1, color);
        if (x, y) == end {
            break;
        }
        let doubled = 2 * error;
        if doubled >= dy {
            error += dy;
            x += sx;
        }
        if doubled <= dx {
            error += dx;
            y += sy;
        }
    }
}

fn set_pixel(pixels: &mut [u8], size: i32, x: i32, y: i32, color: [u8; 4]) {
    if !(0..size).contains(&x) || !(0..size).contains(&y) {
        return;
    }
    let index = ((y * size + x) * 4) as usize;
    pixels[index..index + 4].copy_from_slice(&color);
}
