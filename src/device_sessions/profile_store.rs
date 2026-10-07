//! Explicit physical-profile persistence. Discovery only reads; Save is the
//! sole writer and uses the core's snapshot/backup/publication contract.
use std::path::{Path, PathBuf};
use crate::config::Profile;
use otd_core::storage::{self, FileSnapshot, SaveMode};
use super::DeviceKey;

pub(crate) struct ProfileFile {
    pub path: PathBuf,
    pub snapshot: FileSnapshot,
    pub profile: Option<Profile>,
    pub digest: Option<String>,
    pub error: Option<String>,
}
impl ProfileFile {
    pub fn for_device(key: &DeviceKey) -> Result<Self, String> {
        Self::read(profile_path(&storage::data_directory()?, key)?)
    }
    pub fn read(path: PathBuf) -> Result<Self, String> {
        if let Ok(metadata) = std::fs::metadata(&path) {
            if metadata.len() > crate::control::MAX_PROFILE_BYTES as u64 { return Err("physical profile exceeds 128 KiB".into()); }
        }
        let snapshot = storage::capture(&path)?;
        if !snapshot.exists() { return Ok(Self { path, snapshot, profile: None, digest: None, error: None }); }
        let loaded = storage::read_utf8(&path)?;
        if loaded.text.len() > crate::control::MAX_PROFILE_BYTES { return Err("physical profile exceeds 128 KiB".into()); }
        let digest = Some(sha256(loaded.text.as_bytes())?);
        let (profile, error) = match Profile::from_toml_text(&loaded.text, &path) {
            Ok(profile) => (Some(profile), None), Err(error) => (None, Some(error)),
        };
        Ok(Self { path, snapshot: loaded.snapshot, profile, digest, error })
    }
    pub fn revision(&self) -> Option<u64> { self.profile.as_ref().map(|profile| profile.settings_revision) }
    pub fn matches(&self, profile: &Profile) -> bool {
        self.profile.as_ref().is_some_and(|saved| saved.to_toml().ok().zip(profile.to_toml().ok()).is_some_and(|(a, b)| a == b))
    }
    /// The caller advances a draft once before Save. Exact bytes guard changes
    /// preserving the revision too. No profile is activated by this operation.
    pub fn save(&mut self, profile: &Profile, expected_revision: Option<u64>, expected_digest: Option<&str>) -> Result<Profile, String> {
        if self.revision() != expected_revision || self.digest.as_deref() != expected_digest {
            return Err("physical saved profile changed; reload before saving".into());
        }
        if let Some(revision) = expected_revision {
            if revision.checked_add(1) != Some(profile.settings_revision) { return Err("Save requires exactly the next settings revision".into()); }
        } else if profile.settings_revision == 0 { return Err("advance the settings revision before creating a physical profile".into()); }
        let text = profile.to_toml_at(&self.path)?;
        if text.len() > crate::control::MAX_PROFILE_BYTES { return Err("physical profile exceeds 128 KiB".into()); }
        let saved = Profile::from_toml_text(&text, &self.path)?;
        let digest = sha256(text.as_bytes())?;
        let mode = if self.snapshot.exists() { SaveMode::Replace(&self.snapshot) } else { SaveMode::CreateNew };
        let snapshot = storage::save(&self.path, text.as_bytes(), mode)?;
        self.snapshot = snapshot;
        self.profile = Some(saved.clone());
        self.digest = Some(digest);
        self.error = None;
        Ok(saved)
    }
}
fn profile_path(directory: &Path, key: &DeviceKey) -> Result<PathBuf, String> {
    let mut bytes = b"OTD Rust physical profile v1\0".to_vec();
    for component in [&key.tablet, &key.parent] {
        bytes.extend_from_slice(&(component.len() as u64).to_le_bytes());
        bytes.extend_from_slice(component.as_bytes());
    }
    bytes.push(u8::from(key.fallback));
    Ok(directory.join("devices").join(format!("device-{}.toml", sha256(&bytes)?)))
}
fn sha256(bytes: &[u8]) -> Result<String, String> {
    use windows_sys::Win32::Security::Cryptography::{BCRYPT_HASH_HANDLE, BCRYPT_SHA256_ALG_HANDLE,
        BCryptCreateHash, BCryptHashData, BCryptFinishHash, BCryptDestroyHash};
    struct Hash(BCRYPT_HASH_HANDLE);
    impl Drop for Hash { fn drop(&mut self) { unsafe { BCryptDestroyHash(self.0) }; } }
    let mut raw = std::ptr::null_mut();
    let status = unsafe { BCryptCreateHash(BCRYPT_SHA256_ALG_HANDLE, &mut raw,
        std::ptr::null_mut(), 0, std::ptr::null(), 0, 0) };
    if status != 0 { return Err(format!("physical profile SHA-256 failed: {status:#x}")); }
    let hash = Hash(raw);
    for chunk in bytes.chunks(u32::MAX as usize) {
        let status = unsafe { BCryptHashData(hash.0, chunk.as_ptr(), chunk.len() as u32, 0) };
        if status != 0 { return Err(format!("physical profile SHA-256 failed: {status:#x}")); }
    }
    let mut digest = [0; 32];
    let status = unsafe { BCryptFinishHash(hash.0, digest.as_mut_ptr(), 32, 0) };
    if status != 0 { return Err(format!("physical profile SHA-256 failed: {status:#x}")); }
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn save_survives_restart_and_rejects_same_revision_external_changes() {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        struct Fixture(PathBuf);
        impl Drop for Fixture { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
        let root = Path::new("E:/AgentWork/tmp").join(format!("otd-physical-profile-fixture-{}-{}",
            std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        std::fs::create_dir(&root).unwrap();
        let fixture = Fixture(root);
        let path = fixture.0.join("device.toml");
        let mut file = ProfileFile::read(path.clone()).unwrap();
        let initial = Profile { settings_revision: 1, rotation: 90, ..Profile::default() };
        file.save(&initial, None, None).unwrap();
        let mut reopened = ProfileFile::read(path.clone()).unwrap();
        assert_eq!(reopened.profile.as_ref().unwrap().rotation, 90);
        assert_eq!(reopened.revision(), Some(1));
        let digest = reopened.digest.clone().unwrap();
        assert!(reopened.matches(&initial));
        let draft = Profile { settings_revision: 2, rotation: 270, ..initial.clone() };
        assert!(reopened.save(&draft, Some(0), Some(&digest)).is_err());
        assert!(reopened.save(&draft, Some(1), Some("wrong digest")).is_err());
        // An external editor preserves the revision but changes the bytes.
        // The cached revision/digest match the caller, so the core snapshot
        // guard must catch the conflict before publishing the proposed draft.
        let external = Profile { rotation: 180, ..initial };
        let external_text = external.to_toml_at(&path).unwrap();
        std::fs::write(&path, &external_text).unwrap();
        assert!(reopened.save(&draft, Some(1), Some(&digest)).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), external_text);
        assert_eq!(reopened.profile.as_ref().unwrap().rotation, 90);
        assert!(!reopened.matches(&draft));
        reopened = ProfileFile::read(path.clone()).unwrap();
        assert_ne!(reopened.digest.as_deref(), Some(digest.as_str()));
        let current_digest = reopened.digest.clone().unwrap();
        reopened.save(&draft, Some(1), Some(&current_digest)).unwrap();
        assert_eq!(std::fs::read_to_string(storage::backup_path(&path).unwrap()).unwrap(), external_text);
        let restarted = ProfileFile::read(path).unwrap();
        assert_eq!(restarted.revision(), Some(2));
        assert_eq!(restarted.profile.as_ref().unwrap().rotation, 270);
    }
    #[test]
    fn physical_profile_paths_hide_identity_and_distinguish_identical_models() {
        let key = |parent: &str| DeviceKey { tablet: "same model".into(), parent: parent.into(), fallback: false };
        let root = Path::new("E:/AgentWork/tmp/profile-path-fixture");
        let a = profile_path(root, &key("USB\\serial-a")).unwrap();
        assert_eq!(a, profile_path(root, &key("USB\\serial-a")).unwrap());
        assert_ne!(a, profile_path(root, &key("USB\\serial-b")).unwrap());
        assert!(a.starts_with(root.join("devices")));
        assert!(!a.to_string_lossy().contains("serial-a"));
        assert_eq!(sha256(b"abc").unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
