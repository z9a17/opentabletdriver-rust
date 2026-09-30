//! Panel state: control creation, editing, profiles, the driver thread and
//! the console.
use super::*;

impl App {
    pub(super) fn style(&self) -> Style {
        with_look(|look| look.style).expect("look is initialized")
    }

    pub(super) fn palette(&self) -> Palette {
        self.style().palette
    }

    pub(super) fn s(&self, value: i32) -> i32 {
        scale(value, self.dpi)
    }

    pub(super) fn control(
        &mut self,
        class: &str,
        caption: &str,
        id: u16,
        style: u32,
        kind: Kind,
        surface: Surface,
    ) -> Result<HWND, String> {
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOPARENTNOTIFY,
                wide(class).as_ptr(),
                wide(caption).as_ptr(),
                WS_CHILD | WS_CLIPSIBLINGS | style,
                0,
                0,
                0,
                0,
                self.hwnd,
                id as usize as _,
                GetModuleHandleW(ptr::null()),
                ptr::null(),
            )
        };
        if hwnd.is_null() {
            return Err(format!(
                "cannot create UI control: {}",
                std::io::Error::last_os_error()
            ));
        }
        update_look(|look| {
            look.controls
                .insert(hwnd as isize, ControlInfo { kind, surface });
        });
        let font = if kind == Kind::Memo {
            self.fonts.fonts.mono
        } else {
            self.fonts.fonts.ui
        };
        unsafe {
            SendMessageW(hwnd, WM_SETFONT, font as usize, 0);
            if matches!(kind, Kind::Field | Kind::Memo) {
                SendMessageW(
                    hwnd,
                    EM_SETMARGINS,
                    (EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize,
                    0,
                );
            }
        }
        if matches!(kind, Kind::Memo | Kind::List | Kind::Log) {
            self.dark_mode.apply_control(hwnd, self.palette().dark);
        }
        Ok(hwnd)
    }

    pub(super) fn button(
        &mut self,
        caption: &str,
        id: u16,
        kind: Kind,
        surface: Surface,
    ) -> Result<HWND, String> {
        let tab_stop = if kind == Kind::Menu { 0 } else { WS_TABSTOP };
        self.control(
            "BUTTON",
            caption,
            id,
            tab_stop | BS_PUSHBUTTON as u32,
            kind,
            surface,
        )
    }

    pub(super) fn field(&mut self, id: u16, surface: Surface) -> Result<HWND, String> {
        self.control(
            "EDIT",
            "",
            id,
            WS_TABSTOP | ES_AUTOHSCROLL as u32,
            Kind::Field,
            surface,
        )
    }

    pub(super) fn slider(&mut self, id: u16) -> Result<HWND, String> {
        let hwnd = self.control(
            "msctls_trackbar32",
            "",
            id,
            WS_TABSTOP | TBS_HORZ | TBS_NOTICKS | TBS_BOTH,
            Kind::Slider,
            Surface::Group,
        )?;
        unsafe {
            SendMessageW(hwnd, TBM_SETRANGE, 0, (100 << 16) as isize);
            SendMessageW(hwnd, TBM_SETPAGESIZE, 0, 5);
        }
        Ok(hwnd)
    }

    pub(super) fn create(
        hwnd: HWND,
        prefs: UiPrefs,
        prefs_path: PathBuf,
        process_dpi: isize,
    ) -> Result<Self, String> {
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
        let fonts = FontSet::new(dpi);
        let dark_mode = theme::DarkMode::load();
        let palette = theme::palette_for(prefs.theme);
        dark_mode.apply_app(palette.dark);
        dark_mode.apply_title_bar(hwnd, palette.dark);
        LOOK.with(|slot| {
            *slot.borrow_mut() = Some(Look {
                style: Style {
                    palette,
                    fonts: fonts.fonts,
                    scale: dpi as f32 / 96.0,
                },
                controls: HashMap::new(),
                tab: Tab::Output,
                menu_open: 0,
                filters: Vec::new(),
                log: VecDeque::new(),
                log_columns: [0; 3],
                brushes: RefCell::new(Vec::new()),
            })
        });
        let accelerators = accelerator_table();
        let displays = displays_for_driver(process_dpi).unwrap_or_else(|_| fallback_displays());
        let null = ptr::null_mut();
        let (background_tx, background_rx) = std::sync::mpsc::channel();
        let mut app = Self {
            hwnd,
            dpi,
            fonts,
            dark_mode,
            prefs,
            prefs_path,
            process_dpi,
            tooltip: null,
            filter_tip: vec![0],
            hovered_filter: None,
            accelerators,
            c: Controls {
                menus: Vec::new(),
                tabs: Vec::new(),
                mode: null,
                display: [null; 4],
                tablet: [null; 5],
                relative: [null; 4],
                filter_list: null,
                add_dotnet: null,
                add_native: null,
                remove_filter: null,
                filter_defaults: null,
                property_prev: null,
                property_next: null,
                filter_enable: null,
                tip_binding: null,
                tip_slider: null,
                tip_field: null,
                eraser_binding: null,
                eraser_slider: null,
                eraser_field: null,
                log: null,
                copy_log: null,
                clear_log: null,
                save: null,
                apply: null,
            },
            tab: Tab::Output,
            items: Vec::new(),
            display_view: None,
            tablet_view: None,
            displays,
            editor: Editor::new(Profile::default()),
            profile_path: PathBuf::new(),
            profile_snapshot: None,
            profile_revision_floor: 0,
            recovered_backup: false,
            dirty: false,
            selected_filter: 0,
            properties: Vec::new(),
            plugin_metadata: HashMap::new(),
            metadata_pending: HashMap::new(),
            metadata_versions: HashMap::new(),
            metadata_generation: 0,
            metadata_refresh_deferred: false,
            edit_revision: 0,
            background_tx,
            background_rx,
            device_scan_pending: false,
            device_strings_pending: false,
            import_pending: false,
            diagnostics_pending: false,
            connected_tablets: Vec::new(),
            labels: HashMap::new(),
            property_page: 0,
            invalid: HashSet::new(),
            drag: None,
            context_area: AreaKind::Display,
            running: None,
            daemon_client: None,
            control_busy: false,
            daemon_instance: None,
            daemon_log_sequence: 0,
            closing: false,
            close_ready: false,
            update_restart_pending: false,
            update_close_approved: false,
            driver: DriverState::Stopped,
            tablet_present: None,
            tablet_choices: Vec::new(),
            preset_choices: Vec::new(),
            preset_names: Vec::new(),
            preset_scan_pending: false,
            presets_loaded: false,
            status: String::new(),
            status_level: Level::Info,
            validation_status: false,
            tray_icon: null,
            in_tray: false,
        };
        app.create_controls()?;
        app.create_tooltips();
        app.refresh_presets();
        Ok(app)
    }

    pub(super) fn create_controls(&mut self) -> Result<(), String> {
        for (index, label) in MENUS.iter().enumerate() {
            let hwnd = self.button(label, ID_MENU + index as u16, Kind::Menu, Surface::Window)?;
            self.c.menus.push(hwnd);
        }
        for (index, (tab, label)) in TABS.iter().enumerate() {
            let hwnd = self.button(
                label,
                ID_TAB + index as u16,
                Kind::Tab(*tab),
                Surface::Window,
            )?;
            self.c.tabs.push(hwnd);
        }
        // Creation order is the Tab order: the pages top to bottom, then the bar.
        for (index, (label, _)) in DISPLAY_FIELDS.iter().enumerate() {
            self.c.display[index] = self.labeled_field(ID_DISPLAY + index as u16, label)?;
        }
        for (index, (label, _)) in TABLET_FIELDS.iter().enumerate() {
            self.c.tablet[index] = self.labeled_field(ID_TABLET + index as u16, label)?;
        }
        for (index, (label, _)) in RELATIVE_FIELDS.iter().enumerate() {
            self.c.relative[index] = self.labeled_field(ID_RELATIVE + index as u16, label)?;
        }
        self.c.mode = self.button("Absolute Mode", ID_MODE, Kind::Dropdown, Surface::Page)?;
        self.c.filter_list = self.control(
            "LISTBOX",
            "Filters",
            ID_FILTER_LIST,
            WS_TABSTOP
                | WS_VSCROLL
                | (LBS_OWNERDRAWFIXED | LBS_HASSTRINGS | LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32,
            Kind::List,
            Surface::Group,
        )?;
        self.c.add_dotnet =
            self.button("Add .NET…", CMD_ADD_DOTNET, Kind::Button, Surface::Page)?;
        self.c.add_native =
            self.button("Add native…", CMD_ADD_NATIVE, Kind::Button, Surface::Page)?;
        self.c.remove_filter =
            self.button("Remove", CMD_REMOVE_FILTER, Kind::Button, Surface::Page)?;
        self.c.filter_defaults =
            self.button("Defaults", CMD_FILTER_DEFAULTS, Kind::Button, Surface::Page)?;
        self.c.property_prev =
            self.button("Previous", CMD_PROPERTY_PREV, Kind::Button, Surface::Group)?;
        self.c.property_next =
            self.button("Next", CMD_PROPERTY_NEXT, Kind::Button, Surface::Group)?;
        self.c.filter_enable = self.control(
            "BUTTON",
            "Enable",
            ID_FILTER_ENABLE,
            WS_TABSTOP | BS_AUTOCHECKBOX as u32,
            Kind::Check,
            Surface::Group,
        )?;
        let label = self.label("Tip Binding")?;
        self.c.tip_binding = self.button("Tip", ID_TIP_BINDING, Kind::Dropdown, Surface::Group)?;
        self.labels.insert(self.c.tip_binding as isize, label);
        self.c.tip_field = self.labeled_field(ID_TIP_FIELD, "Tip Threshold")?;
        self.c.tip_slider = self.slider(ID_TIP_SLIDER)?;
        let label = self.label("Eraser Binding")?;
        self.c.eraser_binding =
            self.button("Eraser", ID_ERASER_BINDING, Kind::Dropdown, Surface::Group)?;
        self.labels.insert(self.c.eraser_binding as isize, label);
        self.c.eraser_field = self.labeled_field(ID_ERASER_FIELD, "Eraser Threshold")?;
        self.c.eraser_slider = self.slider(ID_ERASER_SLIDER)?;
        let cue = wide("Tip switch");
        for field in [self.c.tip_field, self.c.eraser_field] {
            unsafe { SendMessageW(field, EM_SETCUEBANNER, 1, cue.as_ptr() as isize) };
        }
        self.c.log = self.control(
            "LISTBOX",
            "Console",
            ID_LOG,
            WS_TABSTOP
                | WS_VSCROLL
                | (LBS_OWNERDRAWFIXED
                    | LBS_HASSTRINGS
                    | LBS_NOTIFY
                    | LBS_NOINTEGRALHEIGHT
                    | LBS_EXTENDEDSEL
                    | LBS_WANTKEYBOARDINPUT) as u32,
            Kind::Log,
            Surface::Group,
        )?;
        self.c.copy_log = self.button("Copy All", CMD_COPY_LOG, Kind::Button, Surface::Page)?;
        self.c.clear_log = self.button("Clear", CMD_CLEAR_LOG, Kind::Button, Surface::Page)?;
        self.c.save = self.button("Save", CMD_SAVE, Kind::Button, Surface::Window)?;
        self.c.apply = self.button("Apply", CMD_APPLY, Kind::Button, Surface::Window)?;
        self.set_item_heights();
        Ok(())
    }

    /// Static text; it names the control created right after it.
    pub(super) fn label(&mut self, text: &str) -> Result<HWND, String> {
        self.control(
            "STATIC",
            text,
            0xFFFF,
            SS_LEFT | SS_CENTERIMAGE | SS_NOPREFIX | SS_ENDELLIPSIS,
            Kind::Label,
            Surface::Group,
        )
    }

    pub(super) fn labeled_field(&mut self, id: u16, text: &str) -> Result<HWND, String> {
        let label = self.label(text)?;
        let field = self.field(id, Surface::Group)?;
        self.labels.insert(field as isize, label);
        Ok(field)
    }

    pub(super) fn set_item_heights(&self) {
        unsafe {
            SendMessageW(self.c.filter_list, LB_SETITEMHEIGHT, 0, self.s(44) as isize);
            SendMessageW(self.c.log, LB_SETITEMHEIGHT, 0, self.s(22) as isize);
        }
    }

    pub(super) fn static_controls(&self) -> Vec<HWND> {
        let c = &self.c;
        let mut all = vec![
            c.mode,
            c.filter_list,
            c.add_dotnet,
            c.add_native,
            c.remove_filter,
            c.filter_defaults,
            c.property_prev,
            c.property_next,
            c.filter_enable,
            c.tip_binding,
            c.tip_slider,
            c.tip_field,
            c.eraser_binding,
            c.eraser_slider,
            c.eraser_field,
            c.log,
            c.copy_log,
            c.clear_log,
            c.save,
            c.apply,
        ];
        all.extend_from_slice(&c.menus);
        all.extend_from_slice(&c.tabs);
        all.extend_from_slice(&c.display);
        all.extend_from_slice(&c.tablet);
        all.extend_from_slice(&c.relative);
        all.extend(self.labels.values().copied());
        all
    }

    pub(super) fn add_tool(&self, target: HWND, id: usize, text: &str) {
        let mut text = wide(text);
        let info = TTTOOLINFOW {
            cbSize: size_of::<TTTOOLINFOW>() as u32,
            uFlags: TTF_SUBCLASS | if id == 0 { TTF_IDISHWND } else { 0 },
            hwnd: self.hwnd,
            uId: if id == 0 { target as usize } else { id },
            lpszText: text.as_mut_ptr(),
            ..Default::default()
        };
        unsafe { SendMessageW(self.tooltip, TTM_ADDTOOLW, 0, &info as *const _ as isize) };
    }

    pub(super) fn remove_tool(&self, target: HWND) {
        let info = TTTOOLINFOW {
            cbSize: size_of::<TTTOOLINFOW>() as u32,
            hwnd: self.hwnd,
            uId: target as usize,
            ..Default::default()
        };
        unsafe { SendMessageW(self.tooltip, TTM_DELTOOLW, 0, &info as *const _ as isize) };
    }

    pub(super) fn set_tool_rect(&self, id: usize, rect: RECT) {
        let info = TTTOOLINFOW {
            cbSize: size_of::<TTTOOLINFOW>() as u32,
            hwnd: self.hwnd,
            uId: id,
            rect,
            ..Default::default()
        };
        unsafe {
            SendMessageW(
                self.tooltip,
                TTM_NEWTOOLRECTW,
                0,
                &info as *const _ as isize,
            )
        };
    }

    pub(super) fn set_tool_text(&self, id: usize, text: &str) {
        let mut text = wide(text);
        let info = TTTOOLINFOW {
            cbSize: size_of::<TTTOOLINFOW>() as u32,
            hwnd: self.hwnd,
            uId: id,
            lpszText: text.as_mut_ptr(),
            ..Default::default()
        };
        unsafe {
            SendMessageW(
                self.tooltip,
                TTM_UPDATETIPTEXTW,
                0,
                &info as *const _ as isize,
            )
        };
    }

    pub(super) fn create_tooltips(&mut self) {
        self.tooltip = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST,
                wide("tooltips_class32").as_ptr(),
                ptr::null(),
                WS_POPUP | TTS_ALWAYSTIP | TTS_NOPREFIX,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                self.hwnd,
                ptr::null_mut(),
                GetModuleHandleW(ptr::null()),
                ptr::null(),
            )
        };
        if self.tooltip.is_null() {
            return;
        }
        self.dark_mode
            .apply_control(self.tooltip, self.palette().dark);
        unsafe { SendMessageW(self.tooltip, TTM_SETMAXTIPWIDTH, 0, self.s(460) as isize) };
        // Upstream's area editor tool tips.
        self.add_tool(self.hwnd, 1, "You can right click the area editor to set the area to a display, adjust alignment, or resize the area.");
        self.add_tool(self.hwnd, 2, "You can right click the area editor to enable aspect ratio locking, adjust alignment, or resize the area.");
        self.add_tool(self.hwnd, 3, "Driver status");
        let filter_tip = TTTOOLINFOW {
            cbSize: size_of::<TTTOOLINFOW>() as u32,
            uFlags: TTF_SUBCLASS | TTF_IDISHWND,
            hwnd: self.hwnd,
            uId: self.c.filter_list as usize,
            lpszText: -1isize as *mut u16,
            ..Default::default()
        };
        unsafe { SendMessageW(self.tooltip, TTM_ADDTOOLW, 0, &filter_tip as *const _ as isize) };
        let c = &self.c;
        let unit = |index: usize, unit: &str| match index {
            0 => format!("Area width in {unit}"),
            1 => format!("Area height in {unit}"),
            2 => format!("Area center X offset in {unit}"),
            _ => format!("Area center Y offset in {unit}"),
        };
        let mut tips: Vec<(HWND, String)> = Vec::new();
        for index in 0..4 {
            tips.push((c.display[index], unit(index, "px")));
            tips.push((c.tablet[index], unit(index, "mm")));
        }
        tips.push((
            c.tablet[4],
            "Angle of rotation about the center of the area.".into(),
        ));
        tips.push((c.relative[0], "Horizontal pointer movement per millimetre of pen movement, before Windows pointer speed and acceleration.".into()));
        tips.push((c.relative[1], "Vertical pointer movement per millimetre of pen movement, before Windows pointer speed and acceleration.".into()));
        tips.push((
            c.relative[2],
            "Angle of rotation applied to pen movement.".into(),
        ));
        tips.push((
            c.relative[3],
            "Lifting the pen for longer than this starts the next movement from the new position."
                .into(),
        ));
        for field in [c.tip_field, c.tip_slider, c.eraser_field, c.eraser_slider] {
            tips.push((
                field,
                "The minimum threshold in order for the assigned binding to activate.".into(),
            ));
        }
        tips.push((
            c.tip_binding,
            "Tip presses the left mouse button in mouse output modes.".into(),
        ));
        tips.push((
            c.eraser_binding,
            "Eraser presses the left mouse button in mouse output modes.".into(),
        ));
        tips.push((
            c.add_dotnet,
            "Add filters from an unchanged OpenTabletDriver .NET plugin DLL.".into(),
        ));
        tips.push((
            c.add_native,
            "Add a native Rust filter DLL (otd_filter_v1).".into(),
        ));
        tips.push((
            c.apply,
            "Restart the running driver with the current settings.".into(),
        ));
        tips.push((c.save, "Save the profile.".into()));
        for (hwnd, tip) in tips {
            self.add_tool(hwnd, 0, &tip);
        }
    }

    // ----- Synchronizing controls with the editor ------------------------------

    pub(super) fn sync_all(&mut self) {
        if let Err(error) = self.editor.update_detected_tablet(&self.connected_tablets) {
            self.log(Level::Warning, "Tablet", format!("Could not use the detected tablet's dimensions: {error}"));
        }
        self.invalid.clear();
        self.sync_mode();
        self.sync_areas(None);
        self.sync_relative(None);
        self.sync_pen(None);
        self.refresh_filters();
        self.update_save_tip();
    }

    pub(super) fn sync_mode(&self) {
        let label = match self.editor.mode() {
            OutputMode::Absolute if self.editor.pen() => PEN_MODE_LABEL,
            OutputMode::Absolute => "Absolute Mode",
            OutputMode::Relative => "Relative Mode",
        };
        set_text(self.c.mode, label);
    }

    pub(super) fn sync_areas(&mut self, skip: Option<HWND>) {
        let mapping = self.editor.absolute(&self.displays);
        let display = mapping.display;
        let tablet = mapping.tablet;
        let values = [
            (self.c.display[0], display.width),
            (self.c.display[1], display.height),
            (self.c.display[2], display.x),
            (self.c.display[3], display.y),
            (self.c.tablet[0], tablet.width),
            (self.c.tablet[1], tablet.height),
            (self.c.tablet[2], tablet.x),
            (self.c.tablet[3], tablet.y),
            (self.c.tablet[4], tablet.rotation),
        ];
        for (index, (hwnd, value)) in values.into_iter().enumerate() {
            if Some(hwnd) != skip {
                // Pixels to 3 decimals, millimetres to 5 (upstream stores floats).
                set_text(
                    hwnd,
                    &model::format_number(value, if index < 4 { 3 } else { 5 }),
                );
                self.set_invalid(hwnd, false, None);
            }
        }
        self.invalidate_areas();
    }

    pub(super) fn sync_relative(&mut self, skip: Option<HWND>) {
        let relative = self.editor.relative();
        let values = [
            relative.sensitivity.0,
            relative.sensitivity.1,
            relative.rotation,
            relative.reset_delay.as_secs_f64() * 1_000.0,
        ];
        for (hwnd, value) in self.c.relative.into_iter().zip(values) {
            if Some(hwnd) != skip {
                set_text(hwnd, &model::format_number(value, 4));
                self.set_invalid(hwnd, false, None);
            }
        }
    }

    pub(super) fn sync_pen(&mut self, skip: Option<HWND>) {
        for eraser in [false, true] {
            let (binding, slider, field) = if eraser {
                (
                    self.c.eraser_binding,
                    self.c.eraser_slider,
                    self.c.eraser_field,
                )
            } else {
                (self.c.tip_binding, self.c.tip_slider, self.c.tip_field)
            };
            let name = if eraser { "Eraser" } else { "Tip" };
            set_text(
                binding,
                if self.editor.binding_enabled(eraser) {
                    name
                } else {
                    "None"
                },
            );
            let percent = self.editor.threshold_percent(eraser);
            if Some(slider) != skip {
                unsafe {
                    SendMessageW(
                        slider,
                        TBM_SETPOS,
                        1,
                        percent.unwrap_or(0.0).round() as isize,
                    );
                    InvalidateRect(slider, ptr::null(), 0);
                }
            }
            if Some(field) != skip {
                set_text(
                    field,
                    &percent
                        .map(|p| model::format_number(p, 2))
                        .unwrap_or_default(),
                );
                self.set_invalid(field, false, None);
            }
        }
    }

    fn ensure_plugin_metadata(&mut self) {
        let paths: HashSet<PathBuf> = self
            .editor
            .profile
            .plugins
            .iter()
            .filter(|plugin| plugin.kind.is_managed())
            .map(|plugin| plugin.path.clone())
            .collect();
        for path in paths {
            if self.plugin_metadata.contains_key(&path) {
                continue;
            }
            self.inspect_plugin(path, false);
        }
    }

    pub(super) fn metadata_for(&self, plugin: &PluginConfig) -> Option<&FilterMetadata> {
        self.plugin_metadata
            .get(&plugin.path)?
            .as_ref()
            .ok()?
            .iter()
            .find(|metadata| metadata.type_name == plugin.type_name)
    }

    pub(super) fn refresh_filters(&mut self) {
        self.refresh_filter_list();
        self.rebuild_properties();
    }

    pub(super) fn filter_tooltip(&mut self, tip: &mut NMTTDISPINFOW) {
        if tip.hdr.hwndFrom != self.tooltip || tip.hdr.idFrom != self.c.filter_list as usize {
            return;
        }
        let mut point = POINT::default();
        unsafe { GetCursorPos(&mut point); ScreenToClient(self.c.filter_list, &mut point); }
        let item = unsafe {
            SendMessageW(self.c.filter_list, LB_ITEMFROMPOINT, 0,
                ((point.y as u32 & 0xffff) << 16 | (point.x as u32 & 0xffff)) as isize)
        };
        let text = if item < 0 || (item as usize >> 16) != 0 {
            String::new()
        } else {
            let index = item as usize & 0xffff;
            with_look(|look| look.filters.get(index).cloned()).flatten().map_or_else(String::new, |filter| {
                let mut text = format!("{}\n{}\n{}", filter.name, filter.detail,
                    if filter.enabled { "Enabled" } else { "Disabled" });
                if let FilterRef::Plugin(index) = filter.target {
                    let Some(plugin) = self.editor.profile.plugins.get(index) else { return String::new(); };
                    text.push_str(&format!("\n{}\n{}", plugin.type_name, plugin.path.display()));
                    if let Some(metadata) = self.metadata_for(plugin) {
                        if let Some(fields) = model::plugin_editor_fields(&plugin.settings_json, Some(metadata)) {
                            let total = fields.len();
                            for field in fields.into_iter().take(12) {
                                let value = if field.value.uses_default() {
                                    field.value.default_cue()
                                } else {
                                    field.value.display_text()
                                };
                                let value: String = value.chars().take(160).collect();
                                text.push_str(&format!("\n{}: {}{}", field.label, value,
                                    if field.unit.is_empty() { String::new() } else { format!(" {}", field.unit) }));
                            }
                            if total > 12 { text.push_str("\nSelect the filter to see all settings."); }
                        }
                    } else {
                        let error = self.plugin_metadata.get(&plugin.path)
                            .and_then(|metadata| metadata.as_ref().err());
                        text.push('\n');
                        text.push_str(error.map_or("Settings information is loading or unavailable.", String::as_str));
                    }
                } else if let FilterRef::Radial(index) = filter.target {
                    text.push_str("\nTablet coordinates, before mapping. No .NET runtime required.");
                    if self.editor.profile.auto_enabled_radial_follow > 0 {
                        text.push_str("\nThis legacy profile auto-enabled imported Radial Follow settings.");
                    }
                    let settings = self.editor.radial(index);
                    for (field, label, unit, _) in RADIAL_FIELDS {
                        text.push_str(&format!("\n{label}: {} {unit}", model::format_number(field.get(&settings), 6)));
                    }
                }
                text
            })
        };
        self.filter_tip = wide(&text);
        tip.lpszText = self.filter_tip.as_mut_ptr();
    }

    pub(super) fn filter_hover_changed(&mut self, window: HWND, point: LPARAM) -> Option<HWND> {
        let hovered = if window == self.c.filter_list {
            let item = unsafe { SendMessageW(window, LB_ITEMFROMPOINT, 0, point) };
            (item >= 0 && (item as usize >> 16) == 0).then_some(item as usize & 0xffff)
        } else { None };
        if hovered == self.hovered_filter { return None; }
        self.hovered_filter = hovered;
        Some(self.tooltip)
    }

    pub(super) fn refresh_deferred_metadata(&mut self) {
        if self.metadata_refresh_deferred && !self.closing && !self.update_restart_pending && !self.editing_controls() {
            self.metadata_refresh_deferred = false;
            self.rebuild_properties();
            self.layout();
        }
    }

    pub(super) fn metadata_changed(&mut self) {
        self.refresh_filter_list();
        self.metadata_refresh_deferred = true;
        self.refresh_deferred_metadata();
    }

    pub(super) fn refresh_filter_list(&mut self) {
        self.hovered_filter = None;
        if self.tab == Tab::Filters {
            self.ensure_plugin_metadata();
        }
        let mut items = self.editor.filters();
        for item in &mut items {
            if let FilterRef::Plugin(index) = item.target
                && let Some(metadata) = self.metadata_for(&self.editor.profile.plugins[index])
                && let Some(name) = metadata
                    .display_name
                    .as_deref()
                    .filter(|name| !name.trim().is_empty())
            {
                item.name = name.to_owned();
            }
        }
        let count = items.len();
        let texts: Vec<Vec<u16>> = items
            .iter()
            .map(|item| {
                let state = if item.enabled { "enabled" } else { "disabled" };
                wide(&format!("{}, {}, {state}", item.name, item.detail))
            })
            .collect();
        update_look(|look| look.filters = items);
        self.selected_filter = self.selected_filter.min(count.saturating_sub(1));
        unsafe {
            SendMessageW(self.c.filter_list, LB_RESETCONTENT, 0, 0);
            for text in &texts {
                SendMessageW(self.c.filter_list, LB_ADDSTRING, 0, text.as_ptr() as isize);
            }
            SendMessageW(self.c.filter_list, LB_SETCURSEL, self.selected_filter, 0);
        }
    }

    pub(super) fn editing_controls(&self) -> bool {
        let focus = unsafe { GetFocus() };
        !self.invalid.is_empty()
            || self.drag.is_some()
            || [self.c.tip_field, self.c.eraser_field].contains(&focus)
            || self.c.display.contains(&focus)
            || self.c.tablet.contains(&focus)
            || self.c.relative.contains(&focus)
            || self.properties.iter().any(|row| row.hwnd == focus)
    }

    pub(super) fn selected_target(&self) -> Option<FilterRef> {
        with_look(|look| {
            look.filters
                .get(self.selected_filter)
                .map(|item| item.target)
        })
        .flatten()
    }

    pub(super) fn rebuild_properties(&mut self) {
        self.metadata_refresh_deferred = false;
        for row in std::mem::take(&mut self.properties) {
            self.remove_tool(row.hwnd);
            self.invalid.remove(&(row.hwnd as isize));
            for hwnd in [Some(row.hwnd), row.label_control]
                .into_iter()
                .flatten()
            {
                self.remove_tool(hwnd);
                update_look(|look| {
                    look.controls.remove(&(hwnd as isize));
                });
                unsafe { DestroyWindow(hwnd) };
            }
        }
        let Some(target) = self.selected_target() else {
            return;
        };
        set_text(self.c.filter_enable, "Enabled");
        unsafe {
            SendMessageW(
                self.c.filter_enable,
                BM_SETCHECK,
                usize::from(self.editor.filter_enabled(target)),
                0,
            );
            EnableWindow(
                self.c.remove_filter,
                matches!(target, FilterRef::Plugin(_)).into(),
            );
            let can_reset = match target {
                FilterRef::Radial(_) => true,
                FilterRef::Plugin(index) => {
                    let plugin = &self.editor.profile.plugins[index];
                    plugin.kind.is_managed() && self.metadata_for(plugin).is_some()
                }
            };
            EnableWindow(self.c.filter_defaults, can_reset.into());
        }
        let mut rows = Vec::new();
        match target {
            FilterRef::Radial(index) => {
                let settings = self.editor.radial(index);
                for (field, label, unit, tip) in RADIAL_FIELDS {
                    rows.push((
                        label.to_owned(),
                        unit.to_owned(),
                        PropertyTarget::Radial(field),
                        Some(tip.to_owned()),
                        model::format_number(field.get(&settings), 6),
                    ));
                }
            }
            FilterRef::Plugin(index) => {
                let plugin = &self.editor.profile.plugins[index];
                let metadata = self.metadata_for(plugin).cloned();
                let properties = model::plugin_editor_fields(&plugin.settings_json, metadata.as_ref())
                    .filter(|properties| properties.len() < MAX_PROPERTY_ROWS as usize);
                match properties {
                    Some(properties) => {
                        for field in properties {
                            let shown = if matches!(&field.value, PropertyValue::JsonScalar | PropertyValue::Json(_)) {
                                "Configured".into()
                            } else {
                                field.value.display_text()
                            };
                            let tooltip = if !field.value.field_writable() {
                                Some("This plugin setting is preserved in the profile and cannot be edited here.".into())
                            } else if let PropertyValue::Typed { .. } = &field.value {
                                let state = if field.value.uses_default() {
                                    field.value.default_cue()
                                } else {
                                    "Changes take effect with Save or Apply.".into()
                                };
                                Some(format!("{}\n\n{state}", field.tooltip.as_deref().unwrap_or("")))
                            } else {
                                field.tooltip
                            };
                            rows.push((
                                field.label,
                                field.unit,
                                PropertyTarget::Plugin(field.key, field.value),
                                tooltip,
                                shown,
                            ));
                        }
                    }
                    None => {}
                }
            }
        }
        // Rows follow the Enable check box in the Tab order.
        let mut after = self.c.filter_enable;
        for (index, (label, unit, target, tip, value)) in rows.into_iter().enumerate() {
            let id = ID_PROPERTY + index as u16;
            let checked = match &target {
                PropertyTarget::Plugin(_, PropertyValue::Bool(value)) => Some(*value),
                _ => None,
            };
            let label_control = if checked.is_none() {
                self.label(&label).ok()
            } else {
                None
            };
            let dropdown =
                matches!(&target, PropertyTarget::Plugin(_, value) if !value.choices().is_empty());
            let created = match checked {
                Some(_) => self.control(
                    "BUTTON",
                    &label,
                    id,
                    WS_TABSTOP | BS_AUTOCHECKBOX as u32,
                    Kind::Check,
                    Surface::Group,
                ),
                None if dropdown => self.button(&value, id, Kind::Dropdown, Surface::Group),
                None => self.field(id, Surface::Group),
            };
            let Ok(hwnd) = created else {
                continue;
            };
            if let PropertyTarget::Plugin(_, value) = &target {
                unsafe {
                    EnableWindow(hwnd, value.field_writable().into());
                }
                if value.uses_default() && !dropdown {
                    unsafe {
                        SendMessageW(
                            hwnd,
                            EM_SETCUEBANNER,
                            0,
                            wide(&value.default_cue()).as_ptr() as isize,
                        );
                    }
                }
            }
            for control in [label_control, Some(hwnd)]
                .into_iter()
                .flatten()
            {
                unsafe {
                    SetWindowPos(
                        control,
                        after,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                }
                after = control;
            }
            match checked {
                Some(value) => unsafe {
                    SendMessageW(hwnd, BM_SETCHECK, usize::from(value), 0);
                },
                None => set_text(hwnd, &value),
            }
            if let Some(tip) = tip {
                self.add_tool(hwnd, 0, &tip);
                if let Some(label) = label_control {
                    self.add_tool(label, 0, &tip);
                }
            }
            self.properties.push(PropertyRow {
                hwnd,
                label_control,
                label,
                unit,
                target,
            });
        }
    }

    pub(super) fn update_save_tip(&self) {
        if self.recovered_backup {
            self.set_tool_text(
                self.c.save as usize,
                "Save recovered settings as a new file; preserve the original and backup",
            );
            return;
        }
        self.set_tool_text(
            self.c.save as usize,
            &format!("Save to {}", self.profile_path.display()),
        );
    }

    pub(super) fn invalidate_areas(&self) {
        for view in [self.display_view, self.tablet_view].into_iter().flatten() {
            unsafe { InvalidateRect(self.hwnd, &view.rect, 0) };
        }
    }

    pub(super) fn invalidate_frame(&self, hwnd: HWND) {
        if let Some(frame) = self.items.iter().find_map(|item| match item {
            Item::Frame(r, h) if *h == hwnd => Some(*r),
            _ => None,
        }) {
            unsafe { InvalidateRect(self.hwnd, &draw::inset(frame, -2, -2), 0) };
        }
    }

    pub(super) fn invalidate_status(&self) {
        let status = self.items.iter().find_map(|item| match item {
            Item::Status(r) => Some(*r),
            _ => None,
        });
        if let Some(r) = status {
            unsafe { InvalidateRect(self.hwnd, &r, 0) };
        }
    }

    // ----- Logging and title ------------------------------------------------------

    pub(super) fn log(&mut self, level: Level, group: &'static str, message: impl Into<String>) {
        let message = message.into();
        self.status = message.clone();
        self.status_level = level;
        self.validation_status = false;
        let mut removed = false;
        let time = local_time();
        let row = wide(&format!("{time} {} {group}: {message}", level.label()));
        update_look(|look| {
            look.log.push_back(LogEntry {
                time,
                level,
                group,
                message,
            });
            if look.log.len() > LOG_LIMIT {
                look.log.pop_front();
                removed = true;
            }
        });
        unsafe {
            if removed {
                SendMessageW(self.c.log, LB_DELETESTRING, 0, 0);
            }
            let index = SendMessageW(self.c.log, LB_ADDSTRING, 0, row.as_ptr() as isize);
            if SendMessageW(self.c.log, LB_GETSELCOUNT, 0, 0) <= 0 && index >= 0 {
                SendMessageW(self.c.log, LB_SETTOPINDEX, index as usize, 0);
            }
        }
        self.set_tool_text(3, &self.status.clone());
        self.invalidate_status();
    }

    pub(super) fn update_title(&self) {
        let mut title = format!("OpenTabletDriver Rust v{}", env!("CARGO_PKG_VERSION"));
        if self.recovered_backup {
            title.push_str(" - Recovered backup (Save As required)");
        }
        if self.driver == DriverState::Connected {
            title.push_str(" - ");
            title.push_str(&self.tablet_label());
        }
        if self.dirty {
            title.insert(0, '*');
        }
        unsafe { SetWindowTextW(self.hwnd, wide(&title).as_ptr()) };
    }

    /// The profile target or the sole detected tablet, never a fixed model.
    pub(super) fn tablet_label(&self) -> String {
        self.editor.tablet_label(&self.connected_tablets)
    }

    /// Makes the profile target a tablet (`None`: whichever is connected).
    pub(super) fn choose_tablet(&mut self, name: Option<String>) {
        if self.editor.profile.target_tablet == name {
            return;
        }
        if let Err(error) = self.editor.set_tablet(name) {
            self.log(Level::Error, "Tablet", format!("Could not select the tablet: {error}. Current settings were kept."));
            return;
        }
        self.mark_dirty();
        self.sync_all();
        self.layout();
        unsafe { InvalidateRect(self.hwnd, ptr::null(), 0) };
        let message = format!(
            "Settings now target {}. Save or Apply to use them.",
            self.tablet_label()
        );
        self.log(Level::Info, "Tablet", message);
    }

    pub(super) fn mark_dirty(&mut self) {
        self.edit_revision = self.edit_revision.wrapping_add(1);
        if !self.dirty {
            self.dirty = true;
            self.update_title();
        }
    }

    // ----- Editing -----------------------------------------------------------------

    pub(super) fn set_invalid(&mut self, hwnd: HWND, invalid: bool, message: Option<&str>) {
        let changed = if invalid {
            self.invalid.insert(hwnd as isize)
        } else {
            self.invalid.remove(&(hwnd as isize))
        };
        if changed {
            self.invalidate_frame(hwnd);
        }
        if let Some(message) = message.filter(|_| invalid) {
            self.show_status(message.into(), Level::Warning, true);
        } else if !invalid && self.invalid.is_empty() && self.validation_status {
            // The input problem is fixed: show the last console message again.
            let last = with_look(|look| {
                look.log
                    .back()
                    .map(|entry| (entry.message.clone(), entry.level))
            })
            .flatten();
            let (message, level) = last.unwrap_or_default();
            self.show_status(message, level, false);
        }
    }

    pub(super) fn show_status(&mut self, message: String, level: Level, validation: bool) {
        self.validation_status = validation;
        if self.status != message || self.status_level != level {
            self.set_tool_text(3, &message);
            self.status = message;
            self.status_level = level;
            self.invalidate_status();
        }
    }

    pub(super) fn field_changed(&mut self, hwnd: HWND) {
        // Count drafts too; invalid text is still a newer user edit.
        self.edit_revision = self.edit_revision.wrapping_add(1);
        let value = text(hwnd);
        if let Some(index) = self.c.display.iter().position(|h| *h == hwnd) {
            self.edit_area(AreaKind::Display, index, hwnd, &value);
        } else if let Some(index) = self.c.tablet.iter().position(|h| *h == hwnd) {
            self.edit_area(AreaKind::Tablet, index, hwnd, &value);
        } else if let Some(index) = self.c.relative.iter().position(|h| *h == hwnd) {
            self.edit_relative(index, hwnd, &value);
        } else if hwnd == self.c.tip_field || hwnd == self.c.eraser_field {
            self.edit_threshold(hwnd == self.c.eraser_field, hwnd, &value);
        } else if let Some(index) = self.properties.iter().position(|row| row.hwnd == hwnd) {
            self.edit_property(index, &value);
        }
    }

    /// Reformats a field from the model once the user leaves it.
    pub(super) fn field_committed(&mut self, hwnd: HWND) {
        if self.c.display.contains(&hwnd) || self.c.tablet.contains(&hwnd) {
            self.sync_areas(None);
        } else if self.c.relative.contains(&hwnd) {
            self.sync_relative(None);
        } else if hwnd == self.c.tip_field || hwnd == self.c.eraser_field {
            self.sync_pen(None);
        }
        if self.metadata_refresh_deferred {
            unsafe { PostMessageW(self.hwnd, WM_METADATA_REFRESH, 0, 0); }
        }
    }

    pub(super) fn bounds(&self, which: AreaKind) -> Bounds {
        match which {
            AreaKind::Display => Bounds::from_rect(self.displays.virtual_screen),
            AreaKind::Tablet => Bounds::tablet_for(self.editor.profile.tablet),
        }
    }

    /// Applies the aspect-ratio and usable-area locks after an area change.
    pub(super) fn commit_mapping(
        &mut self,
        mut mapping: OtdMapping,
        aspect: Option<AspectSource>,
        skip: Option<HWND>,
    ) {
        if self.prefs.lock_aspect_ratio
            && let Some(source) = aspect
        {
            model::lock_aspect(&mut mapping, source);
        }
        if self.prefs.lock_display_to_usable_area {
            model::constrain(&mut mapping.display, self.bounds(AreaKind::Display));
        }
        if self.prefs.lock_tablet_to_usable_area {
            model::constrain(&mut mapping.tablet, self.bounds(AreaKind::Tablet));
        }
        if mapping != self.editor.absolute(&self.displays) {
            self.editor.set_absolute(mapping);
            self.mark_dirty();
        }
        self.sync_areas(skip);
    }

    pub(super) fn edit_area(&mut self, which: AreaKind, index: usize, hwnd: HWND, value: &str) {
        let parsed = model::parse_number(value);
        let Some(value) = parsed.filter(|v| index > 1 || *v > 0.0) else {
            let message = if parsed.is_some() {
                "Enter a size greater than zero."
            } else {
                "Enter a number."
            };
            self.set_invalid(hwnd, true, Some(message));
            return;
        };
        self.set_invalid(hwnd, false, None);
        let mut mapping = self.editor.absolute(&self.displays);
        let previous = (mapping.display.width, mapping.display.height);
        let area = which.area_mut(&mut mapping);
        match index {
            0 => area.width = value,
            1 => area.height = value,
            2 => area.x = value,
            3 => area.y = value,
            _ => area.rotation = value,
        }
        let aspect = match (which, index) {
            (AreaKind::Tablet, 0) => Some(AspectSource::TabletWidth),
            (AreaKind::Tablet, 1) => Some(AspectSource::TabletHeight),
            (AreaKind::Display, 0) => Some(AspectSource::DisplayWidth {
                previous: previous.0,
            }),
            (AreaKind::Display, 1) => Some(AspectSource::DisplayHeight {
                previous: previous.1,
            }),
            _ => None,
        };
        self.commit_mapping(mapping, aspect, Some(hwnd));
    }

    pub(super) fn edit_relative(&mut self, index: usize, hwnd: HWND, value: &str) {
        let Some(value) = model::parse_number(value).filter(|v| index != 3 || *v >= 0.0) else {
            let message = if index == 3 {
                "Enter a reset time of zero or more milliseconds."
            } else {
                "Enter a number."
            };
            self.set_invalid(hwnd, true, Some(message));
            return;
        };
        let mut relative = self.editor.relative();
        match index {
            0 => relative.sensitivity.0 = value,
            1 => relative.sensitivity.1 = value,
            2 => relative.rotation = value,
            _ => match std::time::Duration::try_from_secs_f64(value / 1_000.0) {
                Ok(delay) => relative.reset_delay = delay,
                Err(_) => {
                    self.set_invalid(hwnd, true, Some("The reset time is too large."));
                    return;
                }
            },
        }
        if let Err(error) = relative.validate() {
            self.set_invalid(hwnd, true, Some(&error));
            return;
        }
        self.set_invalid(hwnd, false, None);
        self.editor.set_relative(relative);
        self.mark_dirty();
    }

    pub(super) fn edit_threshold(&mut self, eraser: bool, hwnd: HWND, value: &str) {
        let percent = if value.trim().is_empty() {
            None
        } else {
            match model::parse_number(value).filter(|v| (0.0..=100.0).contains(v)) {
                Some(value) => Some(value),
                None => {
                    self.set_invalid(hwnd, true, Some("Enter a threshold from 0 to 100 percent, or leave it empty to use the tip switch."));
                    return;
                }
            }
        };
        if let Err(error) = self.editor.set_threshold_percent(eraser, percent) {
            self.set_invalid(hwnd, true, Some(&error));
            return;
        }
        self.set_invalid(hwnd, false, None);
        self.mark_dirty();
        self.sync_pen(Some(hwnd));
    }

    pub(super) fn slider_moved(&mut self, slider: HWND) {
        let eraser = slider == self.c.eraser_slider;
        if !eraser && slider != self.c.tip_slider {
            return;
        }
        let position = unsafe { SendMessageW(slider, TBM_GETPOS, 0, 0) } as f64;
        let current = self.editor.threshold_percent(eraser);
        if current.map(f64::round) != Some(position) {
            let _ = self.editor.set_threshold_percent(eraser, Some(position));
            self.mark_dirty();
            self.sync_pen(Some(slider));
        }
        unsafe { InvalidateRect(slider, ptr::null(), 0) };
    }

    pub(super) fn edit_property(&mut self, index: usize, value: &str) {
        let Some(filter) = self.selected_target() else {
            return;
        };
        let hwnd = self.properties[index].hwnd;
        let mut saved_value = None;
        let result = match (&self.properties[index].target, filter) {
            (PropertyTarget::Radial(field), FilterRef::Radial(radial)) => {
                match model::parse_number(value) {
                    Some(number) => {
                        let mut settings = self.editor.radial(radial);
                        field.set(&mut settings, number);
                        self.editor.set_radial(radial, settings);
                        Ok(())
                    }
                    None => Err("Enter a number.".to_owned()),
                }
            }
            (PropertyTarget::Plugin(key, previous), FilterRef::Plugin(plugin)) => {
                model::parse_property(value, previous).and_then(|parsed| {
                    let config = &self.editor.profile.plugins[plugin];
                    let settings_json =
                        model::set_plugin_property(&config.settings_json, key, parsed.clone())?;
                    let candidate = PluginConfig {
                        settings_json,
                        ..config.clone()
                    };
                    candidate.validate()?;
                    self.editor.profile.plugins[plugin] = candidate;
                    saved_value = Some(parsed);
                    Ok(())
                })
            }
            _ => Ok(()),
        };
        match result {
            Ok(()) => {
                if let (
                    Some(value),
                    PropertyTarget::Plugin(_, PropertyValue::Typed { saved, .. }),
                ) = (saved_value, &mut self.properties[index].target)
                {
                    *saved = Some(value);
                }
                self.set_invalid(hwnd, false, None);
                self.update_property_cue(index);
                self.mark_dirty();
            }
            Err(error) => self.set_invalid(hwnd, true, Some(&error)),
        }
    }

    pub(super) fn property_toggled(&mut self, hwnd: HWND) {
        let Some(index) = self.properties.iter().position(|row| row.hwnd == hwnd) else {
            return;
        };
        let checked = unsafe { SendMessageW(hwnd, BM_GETCHECK, 0, 0) } == BST_CHECKED as isize;
        if let (
            PropertyTarget::Plugin(key, PropertyValue::Bool(_)),
            Some(FilterRef::Plugin(plugin)),
        ) = (&self.properties[index].target, self.selected_target())
        {
            let config = &mut self.editor.profile.plugins[plugin];
            if let Ok(updated) =
                model::set_plugin_property(&config.settings_json, key, checked.into())
            {
                let candidate = PluginConfig {
                    settings_json: updated,
                    ..config.clone()
                };
                match candidate.validate() {
                    Ok(()) => {
                        *config = candidate;
                        self.set_invalid(hwnd, false, None);
                        self.mark_dirty();
                    }
                    Err(error) => self.set_invalid(hwnd, true, Some(&error)),
                }
            }
        }
    }

    pub(super) fn choose_property(&mut self, hwnd: HWND, value: serde_json::Value) {
        let Some(index) = self.properties.iter().position(|row| row.hwnd == hwnd) else {
            return;
        };
        let Some(FilterRef::Plugin(plugin)) = self.selected_target() else {
            return;
        };
        let PropertyTarget::Plugin(key, previous) = &self.properties[index].target else {
            return;
        };
        if !previous.writable() {
            return;
        }
        // The default menu action writes a known attribute/editor default.
        let value = if value.is_null() { previous.reset_value() } else { value };
        let config = &self.editor.profile.plugins[plugin];
        let result = model::set_plugin_property(&config.settings_json, key, value.clone())
            .and_then(|settings_json| {
                let candidate = PluginConfig {
                    settings_json,
                    ..config.clone()
                };
                candidate.validate()?;
                Ok(candidate)
            });
        match result {
            Ok(candidate) => {
                self.editor.profile.plugins[plugin] = candidate;
                if let PropertyTarget::Plugin(_, PropertyValue::Typed { saved, .. }) =
                    &mut self.properties[index].target
                {
                    *saved = Some(value);
                }
                if let PropertyTarget::Plugin(_, value) = &self.properties[index].target {
                    set_text(hwnd, &value.display_text());
                }
                self.set_invalid(hwnd, false, None);
                self.update_property_cue(index);
                self.mark_dirty();
            }
            Err(error) => self.set_invalid(hwnd, true, Some(&error)),
        }
    }

    fn update_property_cue(&self, index: usize) {
        let row = &self.properties[index];
        if let PropertyTarget::Plugin(_, value) = &row.target
            && value.choices().is_empty()
        {
            let cue = value.default_cue();
            unsafe {
                SendMessageW(row.hwnd, EM_SETCUEBANNER, 0, wide(&cue).as_ptr() as isize);
            }
        }
    }

    pub(super) fn filter_toggled(&mut self) {
        if !self.can_leave_filter() {
            if let Some(target) = self.selected_target() {
                unsafe {
                    SendMessageW(
                        self.c.filter_enable,
                        BM_SETCHECK,
                        usize::from(self.editor.filter_enabled(target)),
                        0,
                    );
                }
            }
            return;
        }
        let Some(target) = self.selected_target() else {
            return;
        };
        let checked = unsafe { SendMessageW(self.c.filter_enable, BM_GETCHECK, 0, 0) }
            == BST_CHECKED as isize;
        self.editor.set_filter_enabled(target, checked);
        self.mark_dirty();
        self.refresh_filters();
        self.layout();
    }

    pub(super) fn select_filter(&mut self) {
        let index = unsafe { SendMessageW(self.c.filter_list, LB_GETCURSEL, 0, 0) };
        if index >= 0 && index as usize != self.selected_filter {
            if !self.can_leave_filter() {
                unsafe {
                    SendMessageW(self.c.filter_list, LB_SETCURSEL, self.selected_filter, 0);
                }
                return;
            }
            self.selected_filter = index as usize;
                self.property_page = 0;
            self.rebuild_properties();
            self.layout();
        }
    }

    pub(super) fn remove_filter(&mut self) {
        if let Some(FilterRef::Plugin(index)) = self.selected_target() {
            let removed = self.editor.profile.plugins.remove(index);
            self.mark_dirty();
            self.refresh_filters();
            self.layout();
            self.log(
                Level::Info,
                "Plugins",
                format!("Removed {}.", model::plugin_name(&removed)),
            );
        }
    }

    pub(super) fn can_leave_filter(&mut self) -> bool {
        if self
                .properties
                .iter()
                .any(|row| self.invalid.contains(&(row.hwnd as isize)))
        {
            self.show_status(
                "Correct invalid filter edits or use Defaults before changing the editor view."
                    .into(),
                Level::Warning,
                true,
            );
            false
        } else {
            true
        }
    }

    pub(super) fn reset_filter(&mut self) {
        let Some(target) = self.selected_target() else {
            return;
        };
        let metadata = match target {
            FilterRef::Plugin(index) => self
                .metadata_for(&self.editor.profile.plugins[index])
                .cloned(),
            FilterRef::Radial(_) => None,
        };
        match self.editor.reset_filter(target, metadata.as_ref()) {
            Ok(()) => {
                // Defaults intentionally replace invalid property edits.
                        self.mark_dirty();
                self.refresh_filters();
                self.layout();
                self.log(
                    Level::Info,
                    "Plugins",
                    "Restored selected filter defaults. Use Apply to update the running driver.",
                );
            }
            Err(error) => self.log(Level::Warning, "Plugins", error),
        }
    }

    pub(super) fn set_output_mode(&mut self, mode: OutputMode, pen: bool) {
        let pen = pen && mode == OutputMode::Absolute;
        if self.editor.mode() != mode || self.editor.pen() != pen {
            self.editor.set_mode(mode, &self.displays);
            self.editor.set_pen(pen, &self.displays);
            self.mark_dirty();
            self.sync_mode();
            self.sync_areas(None);
            self.sync_relative(None);
            self.layout();
        }
    }

    pub(super) fn set_binding(&mut self, eraser: bool, enabled: bool) {
        if self.editor.binding_enabled(eraser) != enabled {
            self.editor.set_binding_enabled(eraser, enabled);
            self.mark_dirty();
            self.sync_pen(None);
        }
    }

    // ----- Area interaction ---------------------------------------------------------

    pub(super) fn view(&self, which: AreaKind) -> Option<AreaView> {
        match which {
            AreaKind::Display => self.display_view,
            AreaKind::Tablet => self.tablet_view,
        }
    }

    pub(super) fn area_at(&self, point: (i32, i32)) -> Option<(AreaKind, bool)> {
        if self.tab != Tab::Output || self.editor.mode() != OutputMode::Absolute {
            return None;
        }
        let mapping = self.editor.absolute(&self.displays);
        [AreaKind::Display, AreaKind::Tablet]
            .into_iter()
            .find_map(|which| {
                let view = self.view(which)?;
                view.contains(point)
                    .then(|| (which, view.hit(&which.area(&mapping), point)))
            })
    }

    pub(super) fn mouse_down(&mut self, point: (i32, i32)) -> bool {
        let Some((which, true)) = self.area_at(point) else {
            return false;
        };
        let area = which.area(&self.editor.absolute(&self.displays));
        self.drag = Some(Drag {
            which,
            start: point,
            center: (area.x, area.y),
        });
        true
    }

    pub(super) fn mouse_move(&mut self, point: (i32, i32)) {
        let Some(drag) = &self.drag else {
            return;
        };
        let (which, start, center) = (drag.which, drag.start, drag.center);
        let Some(view) = self.view(which) else {
            return;
        };
        let mut mapping = self.editor.absolute(&self.displays);
        let area = which.area_mut(&mut mapping);
        let x = center.0 + f64::from(point.0 - start.0) / view.scale();
        let y = center.1 + f64::from(point.1 - start.1) / view.scale();
        // Whole pixels for displays, micrometres for the tablet.
        let (x, y) = match which {
            AreaKind::Display => (x.round(), y.round()),
            AreaKind::Tablet => (
                (x * 1_000.0).round() / 1_000.0,
                (y * 1_000.0).round() / 1_000.0,
            ),
        };
        if (area.x, area.y) != (x, y) {
            area.x = x;
            area.y = y;
            self.commit_mapping(mapping, None, None);
        }
    }

    pub(super) fn area_menu(&mut self, point: (i32, i32)) -> Option<HMENU> {
        let (which, _) = self.area_at(point)?;
        self.context_area = which;
        let mapping = self.editor.absolute(&self.displays);
        unsafe {
            let menu = CreatePopupMenu();
            let align = CreatePopupMenu();
            for (offset, label) in ["Left", "Right", "Top", "Bottom", "Center"]
                .iter()
                .enumerate()
            {
                append(align, MF_STRING, AREA_ALIGN + offset as u16, label);
            }
            AppendMenuW(menu, MF_POPUP, align as usize, wide("Align").as_ptr());
            let resize = CreatePopupMenu();
            append(resize, MF_STRING, AREA_FULL, "Full area");
            append(resize, MF_STRING, AREA_QUARTER, "Quarter area");
            AppendMenuW(menu, MF_POPUP, resize as usize, wide("Resize").as_ptr());
            let flip = CreatePopupMenu();
            append(flip, MF_STRING, AREA_FLIP_H, "Horizontal");
            append(flip, MF_STRING, AREA_FLIP_V, "Vertical");
            if which == AreaKind::Tablet {
                append(flip, MF_STRING, AREA_HANDEDNESS, "Handedness");
            }
            AppendMenuW(menu, MF_POPUP, flip as usize, wide("Flip").as_ptr());
            let locked = match which {
                AreaKind::Display => self.prefs.lock_display_to_usable_area,
                AreaKind::Tablet => self.prefs.lock_tablet_to_usable_area,
            };
            append(
                menu,
                checked(locked),
                AREA_LOCK_USABLE,
                "Lock to usable area",
            );
            AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null());
            match which {
                AreaKind::Display => {
                    let displays = CreatePopupMenu();
                    let v = self.displays.virtual_screen;
                    append(
                        displays,
                        MF_STRING,
                        AREA_DISPLAY,
                        &format!(
                            "Virtual Display ({}x{}@<{}, {}>)",
                            v.width(),
                            v.height(),
                            v.left,
                            v.top
                        ),
                    );
                    for (index, m) in self.displays.monitors.iter().enumerate() {
                        append(
                            displays,
                            MF_STRING,
                            AREA_DISPLAY + 1 + index as u16,
                            &format!(
                                "Display {} ({}x{}@<{}, {}>)",
                                index + 1,
                                m.width(),
                                m.height(),
                                m.left,
                                m.top
                            ),
                        );
                    }
                    AppendMenuW(
                        menu,
                        MF_POPUP,
                        displays as usize,
                        wide("Set to display").as_ptr(),
                    );
                }
                AreaKind::Tablet => {
                    append(menu, MF_STRING, AREA_CONVERT, "Convert area from...");
                    append(
                        menu,
                        checked(self.prefs.lock_aspect_ratio),
                        AREA_LOCK_ASPECT,
                        "Lock aspect ratio",
                    );
                    append(
                        menu,
                        checked(mapping.clipping),
                        AREA_CLIPPING,
                        "Clamp input outside area",
                    );
                    append(
                        menu,
                        checked(mapping.limiting),
                        AREA_LIMITING,
                        "Ignore input outside area",
                    );
                }
            }
            Some(menu)
        }
    }

    pub(super) fn area_action(&mut self, command: u16) {
        let which = self.context_area;
        let bounds = self.bounds(which);
        let mut mapping = self.editor.absolute(&self.displays);
        let previous = (mapping.display.width, mapping.display.height);
        let mut aspect = None;
        {
            let area = which.area_mut(&mut mapping);
            match command {
                c if (AREA_ALIGN..AREA_ALIGN + 5).contains(&c) => {
                    let direction = [
                        Align::Left,
                        Align::Right,
                        Align::Top,
                        Align::Bottom,
                        Align::Center,
                    ][(c - AREA_ALIGN) as usize];
                    model::align(area, bounds, direction);
                }
                AREA_FULL => {
                    (area.width, area.height) = (bounds.width(), bounds.height());
                    (area.x, area.y) = bounds.center();
                }
                AREA_QUARTER => {
                    (area.width, area.height) = (bounds.width() / 2.0, bounds.height() / 2.0)
                }
                AREA_FLIP_H => model::flip_horizontal(area, bounds),
                AREA_FLIP_V => model::flip_vertical(area, bounds),
                AREA_HANDEDNESS => model::flip_handedness(area, bounds),
                c if c >= AREA_DISPLAY => {
                    let target = if c == AREA_DISPLAY {
                        Some(self.displays.virtual_screen)
                    } else {
                        self.displays
                            .monitors
                            .get((c - AREA_DISPLAY - 1) as usize)
                            .copied()
                    };
                    if let Some(target) = target {
                        let target = Bounds::from_rect(target);
                        (area.width, area.height) = (target.width(), target.height());
                        (area.x, area.y) = target.center();
                    }
                }
                _ => {}
            }
        }
        match command {
            AREA_LOCK_USABLE => match which {
                AreaKind::Display => self.prefs.lock_display_to_usable_area ^= true,
                AreaKind::Tablet => self.prefs.lock_tablet_to_usable_area ^= true,
            },
            AREA_LOCK_ASPECT => {
                self.prefs.lock_aspect_ratio ^= true;
                aspect = Some(AspectSource::TabletWidth);
            }
            AREA_CLIPPING => mapping.clipping ^= true,
            AREA_LIMITING => mapping.limiting ^= true,
            AREA_FULL | AREA_QUARTER if which == AreaKind::Tablet => {
                if self.prefs.lock_aspect_ratio && command == AREA_FULL {
                    let ratio = mapping.display.width / mapping.display.height;
                    (mapping.tablet.width, mapping.tablet.height) =
                        model::fit_aspect(bounds, ratio);
                } else {
                    aspect = Some(AspectSource::TabletWidth);
                }
            }
            _ if which == AreaKind::Display => {
                aspect = Some(AspectSource::DisplayWidth {
                    previous: previous.0,
                });
            }
            _ => {}
        }
        if matches!(command, AREA_LOCK_USABLE | AREA_LOCK_ASPECT) {
            self.save_prefs();
        }
        if which == AreaKind::Display && aspect.is_some() && self.prefs.lock_aspect_ratio {
            model::lock_aspect(
                &mut mapping,
                AspectSource::DisplayHeight {
                    previous: previous.1,
                },
            );
        }
        self.commit_mapping(mapping, aspect, None);
    }

    // ----- Tabs, theme and DPI ---------------------------------------------------------

    pub(super) fn select_tab(&mut self, tab: Tab) {
        if self.tab == tab {
            return;
        }
        if self.tab == Tab::Filters && !self.can_leave_filter() {
            return;
        }
        self.tab = tab;
        self.drag = None;
        update_look(|look| look.tab = tab);
        if tab == Tab::Filters {
            self.refresh_filters();
        }
        for hwnd in &self.c.tabs {
            unsafe { InvalidateRect(*hwnd, ptr::null(), 0) };
        }
        self.layout();
    }

    pub(super) fn cycle_tab(&mut self, step: isize) {
        let index = TABS.iter().position(|(t, _)| *t == self.tab).unwrap_or(0) as isize;
        let next = (index + step).rem_euclid(TABS.len() as isize) as usize;
        self.select_tab(TABS[next].0);
    }

    pub(super) fn apply_theme(&mut self) {
        let palette = theme::palette_for(self.prefs.theme);
        update_look(|look| {
            look.style.palette = palette;
            for (_, brush) in look.brushes.get_mut().drain(..) {
                unsafe { DeleteObject(brush) };
            }
        });
        self.dark_mode.apply_app(palette.dark);
        self.dark_mode.apply_title_bar(self.hwnd, palette.dark);
        plugin_manager::apply_theme();
        debugger::refresh_theme();
        let mut scrolling = vec![self.c.filter_list, self.c.log];
        if !self.tooltip.is_null() {
            scrolling.push(self.tooltip);
        }
        for hwnd in scrolling {
            self.dark_mode.apply_control(hwnd, palette.dark);
        }
        unsafe {
            RedrawWindow(
                self.hwnd,
                ptr::null(),
                ptr::null_mut(),
                RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN,
            );
        }
    }

    pub(super) fn set_theme(&mut self, mode: ThemeMode) {
        self.prefs.theme = mode;
        self.save_prefs();
        self.apply_theme();
    }

    pub(super) fn set_dpi(&mut self, dpi: u32) {
        if dpi == self.dpi {
            return;
        }
        self.dpi = dpi;
        let fonts = FontSet::new(dpi);
        let mut all = self.static_controls();
        all.extend(
            self.properties
                .iter()
                .flat_map(|row| [Some(row.hwnd), row.label_control])
                .flatten(),
        );
        for hwnd in all {
            let font = fonts.fonts.ui;
            unsafe { SendMessageW(hwnd, WM_SETFONT, font as usize, 0) };
        }
        update_look(|look| {
            look.style.fonts = fonts.fonts;
            look.style.scale = dpi as f32 / 96.0;
        });
        // The previous fonts are released only after every control switched.
        self.fonts = fonts;
        self.set_item_heights();
        unsafe {
            SendMessageW(self.tooltip, TTM_SETMAXTIPWIDTH, 0, self.s(460) as isize);
            RedrawWindow(
                self.hwnd,
                ptr::null(),
                ptr::null_mut(),
                RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
            );
        }
        self.set_icons();
    }

    /// Title bar and taskbar icons at the current DPI.
    pub(super) fn set_icons(&self) {
        let accent = Palette::light().accent;
        let big = unsafe { GetSystemMetricsForDpi(SM_CXICON, self.dpi) };
        let small = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, self.dpi) };
        for (kind, size) in [(ICON_BIG, big), (ICON_SMALL, small)] {
            let icon = canvas::app_icon(size.max(16), accent);
            if icon.is_null() {
                continue;
            }
            let previous =
                unsafe { SendMessageW(self.hwnd, WM_SETICON, kind as usize, icon as isize) };
            if previous != 0 {
                unsafe { DestroyIcon(previous as HICON) };
            }
        }
    }

    pub(super) fn save_prefs(&mut self) {
        let mut placement = WINDOWPLACEMENT {
            length: size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        if unsafe { GetWindowPlacement(self.hwnd, &mut placement) } != 0 {
            let r = placement.rcNormalPosition;
            let dip = |value: i32| (i64::from(value) * 96 / i64::from(self.dpi)) as i32;
            self.prefs.window_size = Some((dip(r.right - r.left), dip(r.bottom - r.top)));
            // A panel minimized into the tray keeps the state it restores to.
            self.prefs.maximized = placement.showCmd == SW_SHOWMAXIMIZED as u32
                || (placement.showCmd == SW_SHOWMINIMIZED as u32
                    && placement.flags & WPF_RESTORETOMAXIMIZED != 0);
        }
        if let Err(error) = self.prefs.save(&self.prefs_path) {
            self.log(
                Level::Warning,
                "UI",
                format!("Could not save panel preferences: {error}"),
            );
        }
    }

    // ----- Profiles -------------------------------------------------------------------

    pub(super) fn replace_profile(&mut self, profile: Profile, path: Option<PathBuf>, dirty: bool) {
        self.editor = Editor::new(profile);
        self.plugin_metadata.clear();
        self.metadata_pending.clear();
        self.metadata_versions.clear();
        self.metadata_generation = self.metadata_generation.wrapping_add(1);
        self.metadata_refresh_deferred = false;
        self.import_pending = false;
        if let Some(path) = path {
            self.profile_path = path;
            self.profile_snapshot = None;
            self.profile_revision_floor = 0;
            self.recovered_backup = false;
        }
        self.dirty = dirty;
        self.selected_filter = 0;
        self.property_page = 0;
        self.drag = None;
        self.sync_all();
        self.layout();
        self.update_title();
        for diagnostic in self.editor.profile.diagnostics.clone() {
            self.log(
                Level::Warning,
                "Settings",
                format!("{}: {}", diagnostic.location, diagnostic.message),
            );
        }
    }

    /// A Save/Apply echo should leave the editor exactly where the user was.
    /// Matching snapshots need no control rebuild or metadata rediscovery.
    fn load_active_profile(&mut self, profile: Profile) {
        if let (Ok(current), Ok(incoming)) = (self.editor.profile.to_toml(), profile.to_toml())
            && current == incoming
        {
            return;
        }
        let target = self.selected_target();
        let selected_plugin = match target {
            Some(FilterRef::Plugin(index)) => self.editor.profile.plugins.get(index).cloned(),
            _ => None,
        };
        let top = unsafe { SendMessageW(self.c.filter_list, LB_GETTOPINDEX, 0, 0) }.max(0) as usize;
        let page = self.property_page;
        self.editor = Editor::new(profile);
        self.metadata_generation = self.metadata_generation.wrapping_add(1);
        self.metadata_pending.clear();
        self.metadata_versions.clear();
        self.metadata_refresh_deferred = false;
        self.import_pending = false;
        if let Some(plugin) = selected_plugin {
            let same = |candidate: &PluginConfig| candidate.kind == plugin.kind
                && candidate.path == plugin.path && candidate.type_name == plugin.type_name;
            let previous = match target { Some(FilterRef::Plugin(index)) => index, _ => 0 };
            let index = if self.editor.profile.plugins.get(previous).is_some_and(same) {
                Some(previous)
            } else {
                self.editor.profile.plugins.iter().position(same)
            };
            if let Some(index) = index {
                self.selected_filter = self.editor.profile.radial_follow.len().max(1) + index;
            }
        } else if let Some(FilterRef::Radial(index)) = target {
            self.selected_filter = index;
        }
        self.property_page = page;
        self.drag = None;
        self.sync_all();
        self.layout();
        let count = self.editor.filters().len();
        unsafe { SendMessageW(self.c.filter_list, LB_SETTOPINDEX, top.min(count.saturating_sub(1)), 0); }
        self.update_title();
    }

    /// The profile exactly as Save and Start will use it.
    pub(super) fn checked_profile(&self) -> Result<Profile, String> {
        if !self.invalid.is_empty() {
            return Err("Correct invalid editor values before saving or applying settings.".into());
        }
        model::validated(&self.editor.profile, &self.profile_path)
    }

    /// Persistence must also work for disconnected displays or other tablets.
    /// Check runtime requirements before stopping an existing worker.
    fn validate_start(&self, profile: &Profile) -> Result<(), String> {
        profile.validate_runtime_tablet()?;
        profile.validate_filter_execution()?;
        if profile.relative.is_none() {
            displays_for_driver(self.process_dpi)?.mapper(profile)?;
        }
        Ok(())
    }

    pub(super) fn load_file(&mut self, path: PathBuf) -> bool {
        let loaded = otd_core::storage::read_utf8(&path).and_then(|loaded| {
            Profile::from_toml_text(&loaded.text, &path).map(|profile| (profile, loaded.snapshot))
        });
        match loaded {
            Ok((profile, snapshot)) => {
                let revision = profile.settings_revision;
                self.replace_profile(profile, Some(path.clone()), false);
                self.profile_snapshot = Some(snapshot);
                self.profile_revision_floor = revision;
                self.log(
                    Level::Info,
                    "Settings",
                    format!("Loaded {}.", path.display()),
                );
                true
            }
            Err(error) => {
                self.log(Level::Error, "Settings", error);
                false
            }
        }
    }

    /// Recovery only changes the editor. Saving requires a new file so the
    /// original primary and known-good backup both remain available.
    pub(super) fn recover_profile_backup(&mut self, primary: PathBuf) {
        let recovered = otd_core::storage::read_backup(&primary)
            .and_then(|loaded| Profile::from_toml_text(&loaded.text, &primary));
        match recovered {
            Ok(profile) => {
                self.replace_profile(profile, Some(primary.clone()), true);
                self.recovered_backup = true;
                self.update_title();
                self.update_save_tip();
                self.log(
                    Level::Warning,
                    "Settings",
                    format!(
                        "Recovered the backup of {} into unsaved settings. The original and backup were not changed. Save opens Save As; choose a new file name to keep the recovered settings.",
                        primary.display()
                    ),
                );
            }
            Err(error) => self.log(
                Level::Error,
                "Settings",
                format!("Could not recover backup: {error}. Current editor settings were kept."),
            ),
        }
    }

    pub(super) fn import_otd(&mut self) {
        let exists = std::env::var_os("LOCALAPPDATA")
            .map(|dir| {
                PathBuf::from(dir)
                    .join("OpenTabletDriver")
                    .join("settings.json")
            })
            .is_some_and(|path| path.exists());
        if !exists {
            self.log(
                Level::Warning,
                "Settings",
                "No OpenTabletDriver settings.json was found.",
            );
            return;
        }
        if self.import_pending {
            return;
        }
        let generation = self.metadata_generation;
        let edit_revision = self.edit_revision;
        self.import_pending =
            self.background("settings-import", move || BackgroundResult::Import {
                generation,
                edit_revision,
                result: crate::hid::connected_tablets()
                    .and_then(|names| Profile::load_connected(None, &names))
                    .map(Box::new),
            });
    }
    pub(super) fn save_to(&mut self, path: PathBuf) {
        self.save_to_mode(path, false);
    }

    pub(super) fn save_as_to(&mut self, path: PathBuf) {
        self.save_to_mode(path, true);
    }

    fn save_to_mode(&mut self, path: PathBuf, create_new: bool) {
        if self.recovered_backup && !create_new {
            self.log(
                Level::Error,
                "Settings",
                "Recovered settings require Save As with a new file name to preserve the original and backup.",
            );
            return;
        }
        let result = self.checked_profile().and_then(|mut profile| {
            let mode = match self.profile_snapshot.as_ref() {
                Some(snapshot) if !create_new && snapshot.matches_path(&path)? => {
                    profile.settings_revision =
                        profile.settings_revision.max(self.profile_revision_floor);
                    otd_core::storage::SaveMode::Replace(snapshot)
                }
                _ => otd_core::storage::SaveMode::CreateNew,
            };
            profile.advance_revision()?;
            save_profile(&path, &profile, mode).map(|snapshot| (profile, snapshot))
        });
        match result {
            Ok((profile, snapshot)) => {
                self.editor.profile.settings_revision = profile.settings_revision;
                self.profile_path = path;
                self.profile_snapshot = Some(snapshot);
                self.profile_revision_floor = profile.settings_revision;
                self.recovered_backup = false;
                self.dirty = false;
                self.update_title();
                self.update_save_tip();
                self.log(
                    Level::Info,
                    "Settings",
                    format!("Saved {}.", self.profile_path.display()),
                );
                // Upstream's Save also applies the settings.
                if self.running.is_some() {
                    self.restart_with(profile);
                }
            }
            Err(error) => self.log(Level::Error, "Settings", format!("Could not save: {error}")),
        }
    }

    // ----- Driver -----------------------------------------------------------------------

    pub(super) fn set_driver_state(&mut self, state: DriverState) {
        let was_connected = self.driver == DriverState::Connected;
        self.driver = state;
        let running = self.running.is_some();
        unsafe {
            EnableWindow(
                self.c.apply,
                (running && !self.control_busy && state != DriverState::Stopping).into(),
            );
        }
        if was_connected != (state == DriverState::Connected) {
            self.update_title();
        }
        self.invalidate_status();
        if self.in_tray {
            tray::set_tip(self.hwnd, &self.tray_tip());
        }
    }

    fn tray_tip(&self) -> String {
        format!(
            "OpenTabletDriver Rust\n{}: {}",
            self.tablet_label(),
            self.driver.label()
        )
    }

    /// Adds the notification-area icon, again after Explorer restarts.
    pub(super) fn add_tray(&mut self) {
        if self.tray_icon.is_null() {
            // The notification area uses the system DPI, not the panel's.
            let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem()) };
            self.tray_icon = canvas::app_icon(size.max(16), Palette::light().accent);
        }
        self.in_tray = tray::add(self.hwnd, self.tray_icon, &self.tray_tip());
    }

    /// Attach on every panel launch, including when automatic driver start is off.
    pub(super) fn connect_daemon(&mut self) {
        if self.daemon_client.is_some() {
            return;
        }
        match client::DaemonClient::new(self.hwnd) {
            Ok(client) => self.daemon_client = Some(client),
            Err(error) => self.log(
                Level::Error,
                "Daemon",
                format!("Could not create daemon client: {error}"),
            ),
        }
    }

    fn submit_control(&mut self, command: client::ClientCommand) -> bool {
        if self.closing || self.update_restart_pending {
            return false;
        }
        if self.control_busy {
            self.log(Level::Warning, "Daemon", "Another daemon request is pending; these settings were not applied. Retry Apply after the status updates.");
            return false;
        }
        self.connect_daemon();
        let result = self
            .daemon_client
            .as_ref()
            .ok_or_else(|| "Daemon client unavailable".to_owned())
            .and_then(|client| client.submit(command));
        match result {
            Ok(()) => {
                self.control_busy = true;
                self.set_driver_state(self.driver);
                true
            }
            Err(error) => {
                self.log(Level::Error, "Daemon", error);
                false
            }
        }
    }

    pub(super) fn auto_start(&mut self) {
        if !self.closing && !self.update_restart_pending && self.running.is_none() && !self.control_busy {
            match self.checked_profile() {
                Ok(profile) => self.start_with_intent(profile, true),
                Err(error) => self.log(Level::Error, "Settings", error),
            }
        }
    }

    pub(super) fn start(&mut self) {
        if self.running.is_some() || self.control_busy {
            return;
        }
        match self.checked_profile() {
            Ok(profile) => self.start_with(profile),
            Err(error) => self.log(Level::Error, "Settings", error),
        }
    }

    pub(super) fn start_with(&mut self, profile: Profile) {
        self.start_with_intent(profile, false);
    }

    fn start_with_intent(&mut self, profile: Profile, automatic: bool) {
        if let Err(error) = self.validate_start(&profile) {
            self.log(Level::Error, "Settings", error);
            return;
        }
        let command = if automatic {
            client::ClientCommand::AutoStart(Box::new(profile))
        } else {
            client::ClientCommand::Start(Box::new(profile))
        };
        if self.submit_control(command) {
            self.set_driver_state(DriverState::Starting);
            self.log(
                Level::Info,
                "Daemon",
                "Connecting to the daemon; an existing driver keeps its active settings.",
            );
        }
    }

    pub(super) fn apply(&mut self) {
        if self.running.is_none() {
            self.log(
                Level::Info,
                "Driver",
                "The driver is not attached. Start driver connects to the daemon.",
            );
            return;
        }
        match self.checked_profile() {
            Ok(profile) => self.restart_with(profile),
            Err(error) => self.log(Level::Error, "Settings", error),
        }
    }

    /// The daemon validates then owns the complete stop/start sequence. Its
    /// generation check rejects stale clients before they can stop newer input.
    pub(super) fn restart_with(&mut self, profile: Profile) {
        if let Err(error) = self.validate_start(&profile) {
            self.log(Level::Error, "Settings", error);
            return;
        }
        let Some(expected) = self
            .running
            .as_ref()
            .map(|running| running.identity.clone())
        else {
            return;
        };
        if self.submit_control(client::ClientCommand::Restart {
            expected,
            profile: Box::new(profile),
        }) {
            self.set_driver_state(DriverState::Stopping);
            self.log(Level::Info, "Daemon", "Restart requested with current settings; the daemon waits for cleanup before starting again.");
        }
    }

    pub(super) fn driver_status(&mut self) {
        if self.update_restart_pending { return; }
        let events = self
            .daemon_client
            .as_ref()
            .map(client::DaemonClient::drain)
            .unwrap_or_default();
        for event in events {
            match event {
                client::ClientEvent::CloseFinished(result) => {
                    match result {
                        Ok(warning) => {
                            if let Some(warning) = warning {
                                self.log(Level::Warning, "Daemon", warning);
                            }
                            self.close_ready = true;
                            self.save_prefs();
                            unsafe { PostMessageW(self.hwnd, WM_CLOSE, 0, 0); }
                        }
                        Err(error) => {
                            self.closing = false;
                            self.control_busy = false;
                            unsafe { EnableWindow(self.hwnd, 1); }
                            plugin_manager::set_restart_pending(false);
                            self.log(Level::Error, "Daemon", error);
                            self.set_driver_state(self.driver);
                        }
                    }
                }
                client::ClientEvent::ActionFinished(result) => {
                    if !self.closing { self.control_busy = false; }
                    if let Err(error) = result {
                        self.log(Level::Error, "Daemon", error);
                    }
                    self.set_driver_state(self.driver);
                }
                client::ClientEvent::DaemonExited(message) => {
                    self.log(Level::Error, "Daemon", message);
                }
                client::ClientEvent::Offline(error) => {
                    self.running = None;
                    self.set_driver_state(DriverState::Disconnected);
                    self.log(if error.is_some() { Level::Warning } else { Level::Info }, "Daemon",
                        error.unwrap_or_else(|| "No daemon is available. Start driver launches it; closing this panel stops tablet input.".into()));
                }
                client::ClientEvent::Snapshot { status, profile } => {
                    let identity = status.identity();
                    if self.daemon_instance.as_deref() != Some(&status.instance) {
                        self.daemon_instance = Some(status.instance.clone());
                        self.daemon_log_sequence = 0;
                    }
                    self.running = client::active(status.state).then_some(Running { identity });
                    if let Some(profile) = profile.filter(|_| !self.closing) {
                        if self.dirty || !self.invalid.is_empty() {
                            self.log(Level::Warning, "Settings", "Daemon configuration changed. Unsaved local edits were kept; Apply deliberately replaces the active configuration.");
                        } else {
                            self.load_active_profile(*profile);
                            self.log(Level::Info, "Settings", "Loaded the daemon's active configuration. Save writes it to the local profile file.");
                        }
                    }
                    let mut state = match status.state {
                        crate::control::DriverState::Stopped => DriverState::Stopped,
                        crate::control::DriverState::Starting => DriverState::Starting,
                        crate::control::DriverState::Running => DriverState::Connected,
                        crate::control::DriverState::Stopping => DriverState::Stopping,
                        crate::control::DriverState::Failed => DriverState::Failed,
                    };
                    let first_sequence =
                        status.log_sequence.saturating_sub(status.logs.len() as u64);
                    for (index, message) in status.logs.into_iter().enumerate() {
                        // Derive current tablet state from the bounded snapshot,
                        // but append each daemon log sequence only once.
                        if status.state == crate::control::DriverState::Running {
                            if message.contains("connected; receiving") {
                                state = DriverState::Connected;
                            } else if message.starts_with("Waiting for")
                                || message.starts_with("Device session stopped")
                            {
                                state = DriverState::Waiting;
                            } else if message.contains("found; opening") {
                                state = DriverState::Connecting;
                            }
                        }
                        let sequence = first_sequence + index as u64 + 1;
                        if sequence > self.daemon_log_sequence {
                            let level = if message.starts_with("Device session stopped")
                                || message.starts_with("Disabled failing plugin")
                            {
                                Level::Warning
                            } else {
                                Level::Info
                            };
                            self.log(level, "Driver", message);
                        }
                    }
                    self.daemon_log_sequence = status.log_sequence;
                    self.set_driver_state(state);
                }
            }
        }
    }

    /// Close waits for driver cleanup without blocking the window thread.
    pub(super) fn begin_close(&mut self) -> bool {
        if self.update_close_approved {
            self.closing = true;
            self.save_prefs();
            return true;
        }
        self.connect_daemon();
        let result = self.daemon_client.as_ref()
            .ok_or_else(|| "Daemon client unavailable; input cleanup could not be confirmed.".to_owned())
            .and_then(|client| client.submit(client::ClientCommand::Close));
        match result {
            Ok(()) => {
                self.closing = true;
                self.control_busy = true;
                self.drag = None;
                debugger::close();
                self.set_driver_state(DriverState::Stopping);
                self.show_status("Stopping the driver before closing…".into(), Level::Info, false);
                unsafe { EnableWindow(self.hwnd, 0); }
                plugin_manager::set_restart_pending(true);
            }
            Err(error) => self.log(Level::Error, "Daemon", error),
        }
        false
    }
    // ----- Console -------------------------------------------------------------------

    /// Every console line, as Copy All copies them.
    pub(super) fn log_text(&self) -> String {
        with_look(|look| {
            look.log
                .iter()
                .map(|e| format!("{} [{}:{}] {}", e.time, e.level.label(), e.group, e.message))
                .collect::<Vec<_>>()
                .join("\r\n")
        })
        .unwrap_or_default()
    }

    pub(super) fn copy_log(&mut self, all: bool) {
        let indices: Vec<usize> = if all {
            with_look(|look| (0..look.log.len()).collect()).unwrap_or_default()
        } else {
            let count = unsafe { SendMessageW(self.c.log, LB_GETSELCOUNT, 0, 0) }.max(0) as usize;
            let mut selected = vec![0i32; count];
            unsafe {
                SendMessageW(
                    self.c.log,
                    LB_GETSELITEMS,
                    count,
                    selected.as_mut_ptr() as isize,
                );
            }
            selected.into_iter().map(|i| i as usize).collect()
        };
        let lines = with_look(|look| {
            indices
                .iter()
                .filter_map(|i| look.log.get(*i))
                .map(|e| format!("{} [{}:{}] {}", e.time, e.level.label(), e.group, e.message))
                .collect::<Vec<_>>()
                .join("\r\n")
        })
        .unwrap_or_default();
        if !lines.is_empty() && !copy_to_clipboard(self.hwnd, &lines) {
            self.log(Level::Warning, "UI", "Could not open the clipboard.");
        }
    }

    pub(super) fn clear_log(&mut self) {
        update_look(|look| look.log.clear());
        unsafe { SendMessageW(self.c.log, LB_RESETCONTENT, 0, 0) };
    }
}
