//! A mounted CE filesystem supplies registry, fonts and exact export names.
//! It supplies no executable kernel, GWES, coredll or driver implementation.
use anyhow::{Context, Result};
use pocket_kernel::{ce_hive::Hive, registry::Registry};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

/// Mount an extracted CE image below a writable device overlay. Call once for
/// the root process; children inherit its live registry and VFS namespace.
pub fn configure_device(
    state: &mut pocket_kernel::KernelState,
    image: Option<&WindowsCeImage>,
    data: &Path,
) -> Result<PathBuf> {
    let data = prepare_device_directory(image.map(|i| i.filesystem.as_path()), data)?;
    let registry_path = data.join("registry.json");
    // An existing snapshot includes user deletions. Reimporting hives at each
    // launch would resurrect them, including Explorer's deleted preferences.
    if !registry_path.exists() {
        if let Some(image) = image {
            image.seed_registry(&mut state.registry)?;
        }
    }
    state.registry.configure_persistence(&registry_path)?;
    // Publish the imported image registry at first boot as well as on exit.
    // Explorer's profile remains usable after a host interruption before it
    // writes a preference or reaches normal process teardown.
    if !registry_path.exists() {
        state.registry.flush()?;
    }
    if let Some(image) = image {
        state
            .vfs
            .mount_read_only_filesystem("\\", &image.filesystem);
    }
    state.vfs.mount_filesystem("\\", data.join("fs"));
    Ok(data)
}

/// Resolve existing ancestors before creating a directory. In particular a
/// missing leaf below a Windows junction must not bypass the image boundary.
fn resolve_pending_path(path: &Path) -> Result<PathBuf> {
    let mut ancestor = std::path::absolute(path)?;
    let mut missing = Vec::new();
    loop {
        match ancestor.canonicalize() {
            Ok(mut resolved) => {
                for component in missing.into_iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // A dangling link is still an existing path, not a directory
                // which this launcher may replace or descend through.
                anyhow::ensure!(
                    ancestor.symlink_metadata().is_err(),
                    "Cannot resolve {}",
                    ancestor.display()
                );
                missing.push(
                    ancestor
                        .file_name()
                        .context("path has no existing ancestor")?
                        .to_os_string(),
                );
                anyhow::ensure!(ancestor.pop(), "path has no existing ancestor");
            }
            Err(error) => {
                return Err(error).with_context(|| format!("resolving {}", ancestor.display()))
            }
        }
    }
}

fn path_contains(parent: &Path, child: &Path) -> bool {
    #[cfg(windows)]
    fn key(path: &Path) -> String {
        let text = path.to_string_lossy().replace('/', "\\");
        let text = if let Some(unc) = text.strip_prefix("\\\\?\\UNC\\") {
            format!("\\\\{unc}")
        } else {
            text.strip_prefix("\\\\?\\").unwrap_or(&text).to_owned()
        };
        text.trim_end_matches('\\').to_lowercase()
    }
    #[cfg(windows)]
    {
        let parent = key(parent);
        let child = key(child);
        child == parent || child.starts_with(&format!("{parent}\\"))
    }
    #[cfg(not(windows))]
    {
        child.starts_with(parent)
    }
}

fn prepare_device_directory(image: Option<&Path>, data: &Path) -> Result<PathBuf> {
    let data = resolve_pending_path(data)?;
    let filesystem = resolve_pending_path(&data.join("fs"))?;
    let directories: Vec<_> = ["Windows", "Temp", "My Documents", "Program Files"]
        .into_iter()
        .map(|name| resolve_pending_path(&filesystem.join(name)))
        .collect::<Result<_>>()?;
    let registry = resolve_pending_path(&data.join("registry.json"))?;
    anyhow::ensure!(
        path_contains(&data, &filesystem) && path_contains(&data, &registry),
        "CE device data escapes its selected folder"
    );
    for directory in &directories {
        anyhow::ensure!(
            path_contains(&filesystem, directory),
            "CE writable directory escapes the device filesystem"
        );
    }
    if let Some(image) = image {
        let image = image.canonicalize()?;
        for path in [&data, &filesystem, &registry]
            .into_iter()
            .chain(directories.iter())
        {
            anyhow::ensure!(
                !path_contains(&image, path),
                "CE writable data must be outside the original image: {}",
                path.display()
            );
        }
        anyhow::ensure!(
            !path_contains(&filesystem, &image),
            "CE writable filesystem must not contain the original image"
        );
    }
    // Validate the complete layout first. No rejected configuration should
    // leave newly created directories in the supplied CE image.
    std::fs::create_dir_all(&data)?;
    for directory in directories {
        std::fs::create_dir_all(directory)?;
    }
    Ok(data)
}

/// Mount an application directory above the device filesystem, with writes at
/// the same guest prefix going only to the private device. Portable Explorer
/// creates Desktop/Favorites/Recent beside its EXE; a broader writable root
/// cannot grant writes through a more specific read-only application mount.
pub fn mount_application(
    vfs: &mut pocket_kernel::vfs::Vfs,
    data: &Path,
    guest_prefix: &str,
    source: &Path,
) -> Result<PathBuf> {
    let prefix = guest_prefix.replace('/', "\\");
    anyhow::ensure!(
        prefix.starts_with('\\')
            && !prefix.contains(':')
            && !prefix.contains('\0')
            && !prefix.split('\\').any(|part| part == "." || part == ".."),
        "Use an absolute CE application folder such as \\Application\\"
    );
    let source = source.canonicalize()?;
    anyhow::ensure!(source.is_dir(), "CE application source must be a directory");
    let filesystem = resolve_pending_path(&data.join("fs"))?;
    let mut overlay = filesystem.clone();
    for component in prefix.split('\\').filter(|part| !part.is_empty()) {
        overlay.push(component);
    }
    let overlay = resolve_pending_path(&overlay)?;
    anyhow::ensure!(
        path_contains(&filesystem, &overlay),
        "CE application data escapes the device filesystem"
    );
    anyhow::ensure!(
        !path_contains(&source, &overlay) && !path_contains(&overlay, &source),
        "CE application data must be separate from the original application folder"
    );
    std::fs::create_dir_all(&overlay)?;
    vfs.mount_read_only_filesystem(&prefix, source);
    vfs.mount_filesystem(&prefix, &overlay);
    Ok(overlay)
}

#[derive(Debug, Clone)]
pub struct WindowsCeImage {
    pub filesystem: PathBuf,
    pub windows: PathBuf,
    pub version: (u32, u32, u32),
    pub ordinals: HashMap<String, HashMap<u16, String>>,
    pub hives: Vec<Hive>,
    pub fonts: Vec<(PathBuf, Arc<Vec<u8>>)>,
}
impl WindowsCeImage {
    pub fn load(path: &Path) -> Result<Self> {
        let path = path
            .canonicalize()
            .context("resolving Windows CE image directory")?;
        let filesystem = if path.join("fs/Windows").is_dir() {
            path.join("fs")
        } else if path.join("Windows").is_dir() {
            path
        } else if path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("Windows"))
        {
            path.parent()
                .context("Windows directory has no filesystem parent")?
                .to_path_buf()
        } else {
            anyhow::bail!("CE image must contain Windows/coredll.dll (or fs/Windows/coredll.dll)");
        };
        let filesystem = filesystem.canonicalize()?;
        let windows = filesystem.join("Windows").canonicalize()?;
        let core = std::fs::read(windows.join("coredll.dll"))
            .context("reading CE coredll export metadata")?;
        let mut version = (6, 0, 0);
        if let Some(resource) = pocket_pe::file_version_resource(&core) {
            if let Some(at) = resource.windows(4).position(|p| p == [0xbd, 4, 0xef, 0xfe]) {
                if let Some(fields) = resource.get(at + 8..at + 16) {
                    let ms = u32::from_le_bytes(fields[..4].try_into().unwrap());
                    let ls = u32::from_le_bytes(fields[4..].try_into().unwrap());
                    version = (ms >> 16, ms & 0xffff, ls >> 16);
                }
            }
        }
        let mut ordinals = HashMap::new();
        ordinals.insert("coredll.dll".into(), pocket_pe::export_ordinals(&core)?);
        for name in [
            "commctrl.dll",
            "aygshell.dll",
            "toolhelp.dll",
            "commdlg.dll",
            "gx.dll",
            "ddraw.dll",
            "ole32.dll",
        ] {
            if let Ok(bytes) = std::fs::read(windows.join(name)) {
                ordinals.insert(
                    name.into(),
                    pocket_pe::export_ordinals(&bytes)
                        .with_context(|| format!("reading {name} ordinal metadata"))?,
                );
            }
        }
        let mut hives = Vec::new();
        for name in ["boot.hv", "default.hv", "user.hv"] {
            match std::fs::read(windows.join(name)) {
                Ok(bytes) => hives.push(
                    pocket_kernel::ce_hive::parse(&bytes)
                        .map_err(anyhow::Error::msg)
                        .with_context(|| format!("reading CE {name}"))?,
                ),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(error).with_context(|| format!("reading CE {name}")),
            }
        }
        let mut fonts = Vec::new();
        for entry in std::fs::read_dir(&windows)? {
            let entry = entry?;
            if entry
                .path()
                .extension()
                .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case("ttf"))
            {
                fonts.push((entry.path(), Arc::new(std::fs::read(entry.path())?)));
            }
        }
        fonts.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(Self {
            filesystem,
            windows,
            version,
            ordinals,
            hives,
            fonts,
        })
    }
    pub fn seed_registry(&self, registry: &mut Registry) -> Result<()> {
        let baseline = registry.isolated_copy();
        let mut staged = baseline.isolated_copy();
        for hive in &self.hives {
            for key in &hive.keys {
                anyhow::ensure!(staged.create_key(key), "CE registry allocation failed");
            }
            for (key, name, value) in &hive.values {
                anyhow::ensure!(
                    staged.set_value(key, name, value.clone()),
                    "CE registry value allocation failed"
                );
            }
        }
        anyhow::ensure!(
            registry.commit_isolated(&baseline, &staged),
            "CE registry import failed"
        );
        Ok(())
    }

    /// Per-process metadata and fonts. Registry import is a separate, once per
    /// device operation so CreateProcess never resets a parent's preferences.
    pub fn configure_process(&self, state: &mut pocket_kernel::KernelState) {
        state.ce.version = Some(self.version);
        state.ce.ordinals = self.ordinals.clone();
        state.ce.system_modules.extend(
            [
                "nk.exe",
                "gwes.dll",
                "device.dll",
                "filesys.dll",
                "fsdmgr.dll",
                "coredll.dll",
                "commctrl.dll",
                "aygshell.dll",
                "gx.dll",
                "ddraw.dll",
                "ole32.dll",
                "oleaut32.dll",
                "winsock.dll",
                "ws2.dll",
                "wininet.dll",
            ]
            .into_iter()
            .map(str::to_owned),
        );
        for (path, bytes) in &self.fonts {
            if let Some(fonts) = pocket_kernel::text::parse_resources((**bytes).clone()) {
                state.gdi.install_fonts(path.clone(), fonts, bytes.len());
            } else {
                log::warn!("CE image font is unsupported: {}", path.display());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocket_kernel::vfs::Vfs;

    #[test]
    fn portable_application_writes_are_private_and_inherited_by_children() {
        let original = tempfile::tempdir().unwrap();
        std::fs::write(original.path().join("Prefs.dat"), b"original").unwrap();
        let device = tempfile::tempdir().unwrap();
        let data = prepare_device_directory(None, device.path()).unwrap();
        let mut parent = Vfs::new();
        parent.mount_filesystem("\\", data.join("fs"));
        let overlay =
            mount_application(&mut parent, &data, "\\Application\\", original.path()).unwrap();
        parent.create_directory("\\Application\\Favorites").unwrap();
        let handle = parent
            .open_file("\\Application\\Prefs.dat", 3, 3, 3, false, 0)
            .unwrap()
            .handle;
        assert_eq!(parent.write(handle, b"modified"), Some(8));
        assert!(parent.close(handle));
        assert_eq!(
            std::fs::read(original.path().join("Prefs.dat")).unwrap(),
            b"original"
        );
        assert!(!original.path().join("Favorites").exists());
        // Portable Explorer uses mixed-case guest names. Check its private
        // directory through CE path resolution on case-sensitive hosts too.
        let favorites = parent.resolve("\\application\\FAVORITES").unwrap();
        assert!(favorites.is_dir());
        assert_eq!(favorites.parent(), Some(overlay.as_path()));

        let mut child = Vfs::new();
        child.inherit_namespace(&parent.namespace_snapshot());
        assert_eq!(
            child.resolve("\\APPLICATION\\favorites"),
            Some(favorites.clone())
        );
        assert_eq!(
            std::fs::read(child.resolve("\\Application\\Prefs.dat").unwrap()).unwrap(),
            b"modified"
        );
        assert!(child.delete_file("\\Application\\Prefs.dat"));
        // resolve also supplies a host destination for a missing file. Test
        // guest reads/enumeration instead of mistaking that path for existence.
        assert!(parent
            .open_file("\\Application\\Prefs.dat", 1, 3, 3, false, 0)
            .is_err());
        assert!(!parent
            .list_dir("\\Application\\")
            .unwrap()
            .iter()
            .any(|e| e.0.eq_ignore_ascii_case("Prefs.dat")));
        let mut reopened = Vfs::new();
        mount_application(&mut reopened, &data, "\\Application\\", original.path()).unwrap();
        assert!(reopened
            .open_file("\\Application\\Prefs.dat", 1, 3, 3, false, 0)
            .is_err());
        assert!(!reopened
            .list_dir("\\Application\\")
            .unwrap()
            .iter()
            .any(|e| e.0.eq_ignore_ascii_case("Prefs.dat")));
        assert_eq!(
            std::fs::read(original.path().join("Prefs.dat")).unwrap(),
            b"original"
        );
    }

    #[test]
    fn rejected_device_paths_do_not_create_directories_in_the_image() {
        let image = tempfile::tempdir().unwrap();
        let data = image.path().join("new/device");
        assert!(prepare_device_directory(Some(image.path()), &data).is_err());
        assert!(!image.path().join("new").exists());

        let container = tempfile::tempdir().unwrap();
        let filesystem = container.path().join("fs");
        std::fs::create_dir(&filesystem).unwrap();
        assert!(prepare_device_directory(Some(&filesystem), container.path()).is_err());
        assert!(!filesystem.join("Windows").exists());
    }

    #[test]
    fn application_data_cannot_alias_the_original_directory_or_escape_the_device() {
        let device = tempfile::tempdir().unwrap();
        let data = prepare_device_directory(None, device.path()).unwrap();
        let original = data.join("fs/Application");
        std::fs::create_dir(&original).unwrap();
        let mut vfs = Vfs::new();
        assert!(mount_application(&mut vfs, &data, "\\Application\\", &original).is_err());
        assert!(
            mount_application(&mut vfs, &data, "\\Application\\..\\Escape", &original).is_err()
        );
        assert!(!data.join("fs/Escape").exists());
    }

    #[cfg(windows)]
    #[test]
    fn missing_device_path_below_a_junction_is_checked_before_creation() {
        let image = tempfile::tempdir().unwrap();
        let links = tempfile::tempdir().unwrap();
        let junction = links.path().join("image-link");
        let output = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command",
                "New-Item -ItemType Junction -Path $env:POCKETHLE_TEST_LINK -Target $env:POCKETHLE_TEST_TARGET -ErrorAction Stop | Out-Null"])
            .env("POCKETHLE_TEST_LINK", &junction).env("POCKETHLE_TEST_TARGET", image.path())
            .output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let rejected = prepare_device_directory(Some(image.path()), &junction.join("new/device"));
        std::fs::remove_dir(&junction).unwrap();
        assert!(rejected.is_err());
        assert!(!image.path().join("new").exists());
        assert!(path_contains(image.path(), &image.path().to_path_buf()));
    }
}
