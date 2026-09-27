//! Background inspection and device discovery, with UI-owned completions.
use super::*;

impl App {
    pub(super) fn background(
        &mut self,
        name: &str,
        work: impl FnOnce() -> BackgroundResult + Send + 'static,
    ) -> bool {
        let sender = self.background_tx.clone();
        let window = self.hwnd as isize;
        match std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                if sender.send(work()).is_ok() {
                    unsafe {
                        PostMessageW(window as HWND, WM_BACKGROUND, 0, 0);
                    }
                }
            }) {
            Ok(_) => true,
            Err(error) => {
                self.log(Level::Error, "Background work", error.to_string());
                false
            }
        }
    }

    pub(super) fn inspect_plugin(&mut self, path: PathBuf, add: bool) {
        if let Some(pending_add) = self.metadata_pending.get_mut(&path) {
            *pending_add |= add;
            return;
        }
        self.metadata_pending.insert(path.clone(), add);
        let generation = self.metadata_generation;
        let inspection_path = path.clone();
        if !self.background("plugin-inspection", move || BackgroundResult::Metadata {
            generation,
            result: crate::dotnet::inspect_details(&inspection_path),
            path: inspection_path,
        }) {
            self.metadata_pending.remove(&path);
        }
    }

    pub(super) fn background_results(&mut self) -> Option<String> {
        let mut dialog = None;
        while let Ok(event) = self.background_rx.try_recv() {
            match event {
                BackgroundResult::Metadata {
                    generation,
                    path,
                    result,
                } => {
                    if generation != self.metadata_generation {
                        continue;
                    }
                    let add = self.metadata_pending.remove(&path).unwrap_or(false);
                    match result {
                        Ok(entries) => {
                            self.plugin_metadata.insert(
                                path.clone(),
                                Ok(entries.iter().map(|entry| entry.metadata.clone()).collect()),
                            );
                            if add {
                                if entries.is_empty() {
                                    self.log(
                                        Level::Warning,
                                        "Plugins",
                                        "This assembly exports no supported filters or tools.",
                                    );
                                } else {
                                    self.complete_plugin_add(
                                        entries
                                            .into_iter()
                                            .map(|entry| PluginConfig {
                                                path: path.clone(),
                                                ..entry.config
                                            })
                                            .collect(),
                                    );
                                }
                            } else {
                                let selected = self.selected_target().is_some_and(|target|
                                    matches!(target, FilterRef::Plugin(index) if self.editor.profile.plugins[index].path == path));
                                self.refresh_filter_list();
                                if selected && !self.editing_controls() {
                                    self.rebuild_properties();
                                    self.layout();
                                }
                            }
                        }
                        Err(error) => {
                            self.plugin_metadata
                                .insert(path.clone(), Err(error.clone()));
                            self.log(
                                Level::Error,
                                "Plugins",
                                format!("Could not inspect {}: {error}", path.display()),
                            );
                        }
                    }
                }
                BackgroundResult::PluginFolder {
                    generation,
                    folder,
                    name,
                    entries,
                    errors,
                } => {
                    if generation != self.metadata_generation {
                        continue;
                    }
                    self.metadata_pending.remove(&folder);
                    if !errors.is_empty() {
                        self.log(
                            Level::Warning,
                            "Plugins",
                            format!(
                                "Some DLLs in {name} could not be inspected:\n{}",
                                errors.join("\n")
                            ),
                        );
                    }
                    if entries.is_empty() && errors.is_empty() {
                        self.log(Level::Warning, "Plugins", format!("{name} exports no supported filters or tools; it may provide bindings or output modes."));
                    } else if !entries.is_empty() {
                        let mut configs = Vec::new();
                        for entry in entries {
                            self.plugin_metadata
                                .entry(entry.config.path.clone())
                                .and_modify(|cached| {
                                    if cached.is_err() {
                                        *cached = Ok(Vec::new());
                                    }
                                })
                                .or_insert_with(|| Ok(Vec::new()));
                            if let Some(Ok(metadata)) =
                                self.plugin_metadata.get_mut(&entry.config.path)
                                && !metadata
                                    .iter()
                                    .any(|value| value.type_name == entry.metadata.type_name)
                            {
                                metadata.push(entry.metadata);
                            }
                            configs.push(entry.config);
                        }
                        self.complete_plugin_add(configs);
                    }
                }
                BackgroundResult::Devices { announce, result } => {
                    self.device_scan_pending = false;
                    match result {
                        Ok(pens) => {
                            self.tablet_present = Some(!pens.is_empty());
                            if announce && pens.is_empty() {
                                self.log(
                                    Level::Warning,
                                    "Tablet",
                                    "No supported tablet was found.",
                                );
                            } else if announce {
                                self.log(
                                    Level::Info,
                                    "Tablet",
                                    format!("Found {}.", pens.join(", ")),
                                );
                            }
                            self.connected_tablets = pens;
                        }
                        Err(error) => self.log(
                            Level::Error,
                            "Tablet",
                            format!("HID discovery failed: {error}"),
                        ),
                    }
                }
                BackgroundResult::Import {
                    generation,
                    edit_revision,
                    result,
                } => {
                    if generation != self.metadata_generation {
                        continue;
                    }
                    self.import_pending = false;
                    if edit_revision != self.edit_revision {
                        self.log(Level::Warning, "Settings", "Settings changed while importing. Your edits were kept; import again to replace them.");
                        continue;
                    }
                    match result {
                        Ok(profile) => {
                            let skipped = profile.ignored_filters;
                            self.replace_profile(*profile, None, true);
                            self.log(Level::Info,"Settings","Imported OpenTabletDriver settings. Its own files were not changed.");
                            if skipped > 0 {
                                self.log(Level::Warning,"Settings",format!("Skipped {skipped} enabled filters. Add their DLLs using Plugins > Add .NET plugin."));
                            }
                        }
                        Err(error) => self.log(Level::Error, "Settings", error),
                    }
                }
                BackgroundResult::Strings(text) => {
                    self.device_strings_pending = false;
                    self.log(Level::Info, "Device strings", text.clone());
                    dialog = Some(text);
                }
            }
        }
        dialog
    }

    pub(super) fn read_device_strings(&mut self) {
        if self.device_strings_pending {
            return;
        }
        self.device_strings_pending = self.background("device-strings", || {
            BackgroundResult::Strings(commands::device_string_report())
        });
    }

    pub(super) fn add_plugin_folder(&mut self, folder: PathBuf, name: String) {
        if self.metadata_pending.contains_key(&folder) {
            return;
        }
        self.metadata_pending.insert(folder.clone(), true);
        let generation = self.metadata_generation;
        let work_folder = folder.clone();
        if !self.background("plugin-folder-inspection", move || {
            let mut entries = Vec::new();
            let mut errors = Vec::new();
            for path in crate::plugin_catalog::dlls(&work_folder) {
                match crate::dotnet::inspect_details(&path) {
                    Ok(found) => entries.extend(found),
                    Err(error) => errors.push(format!("{}: {error}", path.display())),
                }
            }
            BackgroundResult::PluginFolder {
                generation,
                folder: work_folder,
                name,
                entries,
                errors,
            }
        }) {
            self.metadata_pending.remove(&folder);
        }
    }

    pub(super) fn add_plugin(&mut self, path: PathBuf, dotnet: bool) {
        if dotnet {
            self.inspect_plugin(path, true);
            return;
        }
        self.complete_plugin_add(vec![PluginConfig {
            path,
            kind: PluginKind::Native,
            enabled: false,
            type_name: String::new(),
            settings_json: "{}".into(),
        }]);
    }

    fn complete_plugin_add(&mut self, entries: Vec<PluginConfig>) {
        if self.editor.profile.plugins.len() + entries.len() > 32 {
            self.log(
                Level::Error,
                "Plugins",
                "At most 32 plugin entries are supported.",
            );
            return;
        }
        let first = self.editor.filters().len();
        let count = entries.len();
        self.editor.profile.plugins.extend(entries);
        self.mark_dirty();
        if self.editing_controls() {
            self.refresh_filter_list();
        } else {
            self.selected_filter = first;
            self.refresh_filters();
            if self.tab == Tab::Filters {
                self.layout();
            } else {
                self.select_tab(Tab::Filters);
            }
        }
        self.log(Level::Info, "Plugins", format!("Added {count} filter entr{} disabled. Select one, check its settings, then enable it.", if count == 1 { "y" } else { "ies" }));
    }

    pub(super) fn detect_tablet(&mut self) {
        self.refresh_tablets(true);
    }

    pub(super) fn refresh_tablets(&mut self, announce: bool) {
        if self.device_scan_pending {
            return;
        }
        self.device_scan_pending =
            self.background("tablet-discovery", move || BackgroundResult::Devices {
                announce,
                result: crate::hid::enumerate()
                    .map(|devices| {
                        let database = otd_core::tablets::Database::builtin();
                        let mut names: Vec<String> = devices
                            .iter()
                            .filter_map(|device| crate::hid::identify(device, database))
                            .filter(|(_, role, supported)| {
                                *supported && *role == otd_core::tablets::Role::Digitizer
                            })
                            .map(|(name, _, _)| name)
                            .collect();
                        names.sort();
                        names.dedup();
                        names
                    })
                    .map_err(|error| error.to_string()),
            });
    }
}
