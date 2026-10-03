//! The actual discovery completion, layout, paint and mouse paths, using a
//! hidden owner and synthetic device results. No app startup or daemon client.
use super::*;

fn fixture(hwnd: HWND) -> App {
    let null = ptr::null_mut();
    let fonts = FontSet::new(96);
    LOOK.with(|slot| *slot.borrow_mut() = Some(Look {
        style: Style { palette: Palette::dark(), fonts: fonts.fonts, scale: 1.0 },
        controls: HashMap::new(), tab: Tab::Output, menu_open: 0,
        filters: Vec::new(), log: VecDeque::new(), log_columns: [0; 3],
        brushes: RefCell::new(Vec::new()),
    }));
    let (background_tx, background_rx) = std::sync::mpsc::channel();
    App {
        hwnd, dpi: 96, fonts, dark_mode: theme::DarkMode::load(),
        prefs: UiPrefs::default(), prefs_path: PathBuf::new(), process_dpi: 0,
        tooltip: null, filter_tip: vec![0], hovered_filter: None, accelerators: null,
        c: Controls {
            menus: Vec::new(), tabs: vec![null; TABS.len()], mode: null,
            display: [null; 4], tablet: [null; 5], relative: [null; 4],
            filter_list: null, add_dotnet: null, add_native: null, remove_filter: null,
            filter_defaults: null, property_prev: null, property_next: null, filter_enable: null,
            tip_binding: null, tip_slider: null, tip_field: null,
            eraser_binding: null, eraser_slider: null, eraser_field: null,
            log: null, copy_log: null, clear_log: null, save: null, apply: null,
        },
        tab: Tab::Output, items: Vec::new(), display_view: None, tablet_view: None,
        displays: fallback_displays(), editor: Editor::new(Profile::default()),
        profile_path: PathBuf::new(), profile_snapshot: None, profile_revision_floor: 0,
        recovered_backup: false, dirty: false, selected_filter: 0, properties: Vec::new(),
        plugin_metadata: HashMap::new(), metadata_pending: HashMap::new(), metadata_versions: HashMap::new(),
        metadata_generation: 0, metadata_refresh_deferred: false, edit_revision: 0,
        background_tx, background_rx, device_scan: background::DeviceScan::default(),
        device_strings_pending: false, import_pending: false, diagnostics_pending: false,
        connected_tablets: Vec::new(), binding_rows: Vec::new(), wheel_fields: Vec::new(),
        bindings_detected: false, labels: HashMap::new(), property_page: 0,
        invalid: HashSet::new(), drag: None, context_area: AreaKind::Tablet,
        running: None, daemon_client: None, control_busy: false, daemon_instance: None,
        daemon_log_sequence: 0, closing: false, close_ready: false,
        update_restart_pending: false, update_close_approved: false,
        updates: updates::UpdateState::default(), driver: DriverState::Stopped,
        tablet_present: None, tablet_choices: Vec::new(), preset_choices: Vec::new(),
        preset_names: Vec::new(), preset_scan_pending: false, presets_loaded: false,
        status: String::new(), status_level: Level::Info, validation_status: false,
        tray_icon: null, in_tray: false,
    }
}

fn discover(app: &mut App, result: Result<Vec<String>, String>) {
    app.background_tx.send(BackgroundResult::Devices { announce: false, result }).unwrap();
    assert!(app.background_results().is_none());
}

fn render(app: &App, name: &str, blank: bool) {
    let client = client_rect(app.hwnd);
    let mut canvas = canvas::Canvas::new(ptr::null_mut(), client).unwrap();
    app.paint_items(&mut canvas, client);
    let bmp = canvas.to_bmp();
    let width = client.right - client.left;
    let area = app.items.iter().find_map(|item| match item {
        Item::Area(rect, AreaKind::Tablet) => Some(*rect), _ => None,
    }).unwrap();
    let color = app.palette().group;
    let expected = [color.2, color.1, color.0];
    let mut changed = 0;
    for y in area.top..area.bottom {
        for x in area.left..area.right {
            let offset = 54 + ((y * width + x) * 4) as usize;
            if bmp[offset..offset + 3] != expected { changed += 1; }
        }
    }
    if blank {
        assert_eq!(changed, 0, "{name}: no bounds, area, labels, dot or error text may remain");
        let point = ((area.left + area.right) / 2, (area.top + area.bottom) / 2);
        assert_eq!(app.area_at(point), None, "{name}: absent preview must not accept input");
    } else {
        assert!(changed > 100, "{name}: connected preview/error must actually render");
    }
    if let Some(directory) = std::env::var_os("OTD_AREA_TEST_OUTPUT") {
        let directory = PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join(format!("{name}.bmp")), bmp).unwrap();
    }
}

#[test]
fn disconnected_preview_is_blank_through_discovery_layout_paint_and_reconnect() {
    let window = unsafe { CreateWindowExW(0, wide("STATIC").as_ptr(), wide("Area regression").as_ptr(),
        WS_POPUP, 0, 0, 680, 580, ptr::null_mut(), ptr::null_mut(), GetModuleHandleW(ptr::null()), ptr::null()) };
    assert!(!window.is_null());
    assert_eq!(unsafe { IsWindowVisible(window) }, 0);
    let mut app = fixture(window);
    let mut mapping = app.editor.absolute(&app.displays);
    mapping.tablet = crate::mapping::OtdArea { width: 82.6, height: 48.2, x: 41.3, y: 69.0, rotation: 0.0 };
    app.editor.set_absolute(mapping);
    let saved = app.editor.profile.to_toml().unwrap();
    app.layout();
    for (name, palette) in [("startup-dark", Palette::dark()), ("startup-light", Palette::light()),
        ("startup-high-contrast", Palette::high_contrast())] {
        update_look(|look| look.style.palette = palette);
        assert!(app.tablet_view.is_none());
        render(&app, name, true);
    }
    update_look(|look| look.style.palette = Palette::dark());
    discover(&mut app, Ok(Vec::new()));
    render(&app, "absent-scan", true);
    assert_eq!(app.editor.profile.to_toml().unwrap(), saved);

    // Pending edits prevent profile replacement, but not device-bound drawing.
    app.dirty = true;
    discover(&mut app, Ok(vec!["Wacom PTK-470".into()]));
    let view = app.tablet_view.unwrap();
    assert_eq!((view.bounds.width(), view.bounds.height()), (187.0, 105.0));
    assert_eq!(app.editor.profile.tablet, otd_core::spec::TabletSpec::PTH_660);
    assert_eq!(app.editor.profile.to_toml().unwrap(), saved);
    render(&app, "connected-ptk470", false);
    let center = view.project(41.3, 69.0);
    assert!(app.mouse_down((center.0 as i32, center.1 as i32)));
    discover(&mut app, Ok(Vec::new()));
    assert!(app.drag.is_none());
    assert!(app.tablet_view.is_none());
    app.area_action(AREA_FULL);
    assert_eq!(app.editor.profile.to_toml().unwrap(), saved);
    render(&app, "removed-dirty", true);

    // Loading a daemon/profile snapshot while absent cannot revive fallback bounds.
    app.dirty = false;
    let mut snapshot = app.editor.profile.clone();
    snapshot.relative = Some(app.editor.relative());
    app.load_active_profile(snapshot);
    app.load_active_profile(Profile::from_toml_text(&saved, Path::new("offline-profile.toml")).unwrap());
    assert!(app.tablet_view.is_none());
    render(&app, "absent-profile-reload", true);

    discover(&mut app, Ok(vec!["Wacom PTK-470".into()]));
    assert_eq!(app.tablet_view.unwrap().bounds.width(), 187.0);
    render(&app, "reconnected-ptk470", false);
    assert_eq!(app.editor.profile.to_toml().unwrap(), saved);
    discover(&mut app, Err("Synthetic discovery failure".into()));
    assert_eq!(app.tablet_present, None);
    render(&app, "failed-scan", true);

    app.editor.profile.target_tablet = Some("Wacom PTH-660".into());
    discover(&mut app, Ok(vec!["Wacom PTK-470".into()]));
    render(&app, "absent-named-target", true);
    discover(&mut app, Ok(vec!["Wacom PTK-470".into(), "Wacom PTH-660".into()]));
    assert_eq!(app.tablet_view.unwrap().bounds.width(), 224.0);
    render(&app, "connected-named-target", false);
    app.editor.profile.target_tablet = Some("*".into());
    discover(&mut app, Ok(vec!["Wacom PTK-470".into(), "Wacom PTH-660".into()]));
    render(&app, "ambiguous-automatic", true);

    app.editor.profile.otd_mapping.as_mut().unwrap().tablet.width = 0.0;
    app.invalid.insert(12345);
    let invalid = app.editor.profile.to_toml().unwrap();
    discover(&mut app, Ok(vec!["Wacom PTK-470".into()]));
    render(&app, "connected-invalid-mapping", false);
    discover(&mut app, Ok(Vec::new()));
    render(&app, "removed-invalid-mapping", true);
    assert_eq!(app.editor.profile.to_toml().unwrap(), invalid);
    assert_eq!(unsafe { IsWindowVisible(window) }, 0);
    assert!(app.daemon_client.is_none() && app.running.is_none());
    drop(app);
    LOOK.with(|slot| slot.borrow_mut().take());
    unsafe { DestroyWindow(window); }
}


/// Renders the page items and the visible child controls the way the panel
/// draws them (shared custom draw for buttons, palette text for labels) into
/// a BMP under `OTD_AREA_TEST_OUTPUT`, when that is set.
fn render_page(app: &App, name: &str) {
    let Some(directory) = std::env::var_os("OTD_AREA_TEST_OUTPUT") else { return; };
    let client = client_rect(app.hwnd);
    let (width, height) = (client.right, client.bottom);
    let info = BITMAPINFO { bmiHeader: BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: width, biHeight: -height,
        biPlanes: 1, biBitCount: 32, biCompression: BI_RGB, ..Default::default()
    }, ..Default::default() };
    unsafe {
        let dc = CreateCompatibleDC(ptr::null_mut());
        let mut bits = ptr::null_mut();
        let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, ptr::null_mut(), 0);
        let previous = SelectObject(dc, bitmap);
        let mut canvas = canvas::Canvas::new(dc, client).unwrap();
        app.paint_items(&mut canvas, client);
        canvas.present(dc);
        let palette = app.palette();
        for hwnd in app.static_controls().into_iter().chain(app.binding_controls()) {
            if GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_VISIBLE == 0 { continue; }
            let Some(info) = with_look(|look| look.info(hwnd)).flatten() else { continue; };
            let mut bounds = RECT::default();
            GetWindowRect(hwnd, &mut bounds);
            MapWindowPoints(ptr::null_mut(), app.hwnd, (&mut bounds as *mut RECT).cast(), 2);
            let saved = SaveDC(dc);
            SetViewportOrgEx(dc, bounds.left, bounds.top, ptr::null_mut());
            match info.kind {
                Kind::Button | Kind::Dropdown | Kind::Tab(_) | Kind::Menu | Kind::Check => {
                    let mut draw = NMCUSTOMDRAW { hdr: NMHDR { hwndFrom: hwnd, idFrom: 0, code: NM_CUSTOMDRAW },
                        dwDrawStage: CDDS_PREPAINT, hdc: dc, ..Default::default() };
                    paint::custom_draw(&mut draw);
                }
                Kind::Label | Kind::Field => {
                    let mut area = rect(0, 0, bounds.right - bounds.left, bounds.bottom - bounds.top);
                    let text = wide(&text(hwnd));
                    SelectObject(dc, app.style().fonts.ui);
                    SetBkMode(dc, TRANSPARENT as i32);
                    SetTextColor(dc, palette.text.colorref());
                    DrawTextW(dc, text.as_ptr(), -1, &mut area, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
                }
                _ => {}
            }
            RestoreDC(dc, saved);
        }
        GdiFlush();
        let pixels = std::slice::from_raw_parts(bits as *const u32, (width * height) as usize);
        let mut bmp = Vec::new();
        bmp.extend_from_slice(b"BM");
        bmp.extend_from_slice(&(54 + pixels.len() as u32 * 4).to_le_bytes());
        bmp.extend_from_slice(&[0; 4]);
        bmp.extend_from_slice(&54u32.to_le_bytes());
        bmp.extend_from_slice(&40u32.to_le_bytes());
        bmp.extend_from_slice(&width.to_le_bytes());
        bmp.extend_from_slice(&(-height).to_le_bytes());
        bmp.extend_from_slice(&1u16.to_le_bytes());
        bmp.extend_from_slice(&32u16.to_le_bytes());
        bmp.extend_from_slice(&[0; 24]);
        for pixel in pixels { bmp.extend_from_slice(&pixel.to_le_bytes()); }
        let directory = PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join(format!("{name}.bmp")), bmp).unwrap();
        SelectObject(dc, previous);
        DeleteObject(bitmap);
        DeleteDC(dc);
    }
}

/// Window rectangles of the controls a page shows.
fn placed(app: &App, controls: &[HWND]) -> Vec<RECT> {
    controls.iter().filter(|hwnd| unsafe { GetWindowLongW(**hwnd, GWL_STYLE) } as u32 & WS_VISIBLE != 0)
        .map(|hwnd| {
            let mut bounds = RECT::default();
            unsafe {
                GetWindowRect(*hwnd, &mut bounds);
                MapWindowPoints(ptr::null_mut(), app.hwnd, (&mut bounds as *mut RECT).cast(), 2);
            }
            bounds
        }).collect()
}

fn overlaps(a: &RECT, b: &RECT) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

#[test]
fn binding_pages_follow_the_detected_tablet_and_edit_the_profile() {
    use bindings::BindingTarget;
    use otd_core::output::buttons::ButtonAction;
    let window = unsafe { CreateWindowExW(0, wide("STATIC").as_ptr(), wide("Binding pages").as_ptr(),
        WS_POPUP, 0, 0, 760, 640, ptr::null_mut(), ptr::null_mut(), GetModuleHandleW(ptr::null()), ptr::null()) };
    assert!(!window.is_null());
    let mut app = fixture(window);
    // The fixture's placeholder tabs and menus are replaced by real ones.
    app.c.tabs.clear();
    app.c.menus.clear();
    app.create_controls().unwrap();
    discover(&mut app, Ok(vec!["Wacom PTH-660".into()]));
    let targets: Vec<BindingTarget> = app.binding_rows.iter().map(|row| row.target).collect();
    let mut expected = vec![BindingTarget::Pen(0), BindingTarget::Pen(1)];
    expected.extend((0..8).map(BindingTarget::Aux));
    expected.extend([BindingTarget::Clockwise(0), BindingTarget::CounterClockwise(0), BindingTarget::WheelButton(0, 0)]);
    assert_eq!(targets, expected, "two side buttons, eight express keys and the ring");
    assert_eq!(app.wheel_fields.len(), 2);

    for (tab, shown_rows) in [(Tab::Pen, 2), (Tab::Aux, 11)] {
        app.select_tab(tab);
        let rows: Vec<HWND> = app.binding_rows.iter()
            .filter(|row| matches!(row.target, BindingTarget::Pen(_)) == (tab == Tab::Pen))
            .flat_map(|row| [row.hwnd, row.label]).collect();
        let others: Vec<HWND> = app.binding_rows.iter()
            .filter(|row| matches!(row.target, BindingTarget::Pen(_)) != (tab == Tab::Pen))
            .map(|row| row.hwnd).collect();
        let mut controls = rows.clone();
        if tab == Tab::Aux {
            controls.extend(app.wheel_fields.iter().map(|field| field.hwnd));
        }
        let rects = placed(&app, &controls);
        assert_eq!(rects.len(), controls.len(), "{tab:?}: every row and field is placed");
        assert_eq!(rows.len(), shown_rows * 2);
        assert!(placed(&app, &others).is_empty(), "{tab:?}: other rows are hidden");
        let client = client_rect(app.hwnd);
        for (index, a) in rects.iter().enumerate() {
            assert!(a.left >= 0 && a.right <= client.right && a.bottom <= client.bottom - 48, "{tab:?}: {:?} inside the page", (a.left, a.top, a.right, a.bottom));
            for b in &rects[index + 1..] {
                assert!(!overlaps(a, b), "{tab:?}: {:?} overlaps {:?}", (a.left, a.top, a.right, a.bottom), (b.left, b.top, b.right, b.bottom));
            }
        }
        for (name, palette) in [("dark", Palette::dark()), ("light", Palette::light()), ("high-contrast", Palette::high_contrast())] {
            update_look(|look| look.style.palette = palette);
            render_page(&app, &format!("bindings-{tab:?}-{name}"));
        }
        update_look(|look| look.style.palette = Palette::dark());
    }

    // Editing writes the profile and the dropdown text.
    let key = ButtonAction::Keys(otd_core::keys::parse_chord("Control+Z").unwrap());
    app.set_binding_action(BindingTarget::Aux(2), key.clone());
    app.set_binding_action(BindingTarget::Clockwise(0), "keys:PageDown".parse().unwrap());
    assert!(app.dirty);
    assert_eq!(app.editor.profile.aux_buttons, [ButtonAction::None, ButtonAction::None, key]);
    assert_eq!(text(app.binding_rows[4].hwnd), "LeftControl+Z");
    let field = app.wheel_fields[0].hwnd;
    set_text(field, "15");
    app.field_changed(field);
    assert_eq!(app.editor.profile.wheels[0].clockwise_threshold, Some(15.0));
    set_text(field, "-3");
    app.field_changed(field);
    assert!(app.invalid.contains(&(field as isize)));
    assert_eq!(app.editor.profile.wheels[0].clockwise_threshold, Some(15.0), "invalid text keeps the value");
    set_text(field, "");
    app.field_changed(field);
    assert_eq!(app.editor.profile.wheels[0].clockwise_threshold, None, "empty is one wheel step");
    app.select_tab(Tab::Aux);
    render_page(&app, "bindings-Aux-edited");

    // Without a tablet the rows show what the profile binds.
    app.dirty = false;
    discover(&mut app, Ok(Vec::new()));
    let targets: Vec<BindingTarget> = app.binding_rows.iter().map(|row| row.target).collect();
    assert_eq!(targets, [
        BindingTarget::Pen(0), BindingTarget::Pen(1), BindingTarget::Pen(2),
        BindingTarget::Aux(0), BindingTarget::Aux(1), BindingTarget::Aux(2),
        BindingTarget::Clockwise(0), BindingTarget::CounterClockwise(0),
    ]);
    assert!(!app.bindings_detected);
    render_page(&app, "bindings-Aux-absent");
    drop(app);
    LOOK.with(|slot| slot.borrow_mut().take());
    unsafe { DestroyWindow(window); }
}
