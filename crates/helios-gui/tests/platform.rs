//! The desktop's platform services: a second launch, another process,
//! hands its files and links to the running Helios; a long job's end is
//! said on the desktop only while no window has the focus.

use std::process::Command;
use std::time::{Duration, Instant};

use egui_pop::api::App;
use egui_pop::model::commands::Origin;
use egui_pop::model::values::{Change, Set, Value};
use egui_pop::platform::crash::CrashPlugin;
use egui_pop::platform::instance::{self, InstancePlugin, Launch, Single};
use egui_pop::platform::logs::{Logs, LogsPlugin};
use egui_pop::platform::notify::{NotifyPlugin, Simulated};
use helios_gui::platform::{
    Job, JobEnd, LONG_JOB_SECONDS, Open, Opens, Platform, SCHEME, SETTING_JOBS, STATE_NAME, about,
    job_finished, jobs_enabled, should_notify, window_focused,
};

const CHILD: &str = "HELIOS_GUI_PLATFORM_CHILD";

/// The second launch, run by `a_second_launch_hands_over` as a process of
/// its own: it hands its arguments to the instance named in `CHILD`.
#[test]
fn second_launch_child() {
    let Ok(id) = std::env::var(CHILD) else {
        return;
    };
    let launch = Launch {
        args: vec![
            "--vault".into(),
            "ignored.db".into(),
            "photo.jpg".into(),
            "helios://open?path=/shoot/raw%20one.dng".into(),
        ],
        cwd: Some("/shoot".into()),
        activation: None,
    };
    assert!(matches!(
        instance::single_with(&id, launch).unwrap(),
        Single::Handed
    ));
}

#[cfg(unix)]
#[test]
fn a_second_launch_hands_over() {
    let id = format!("io.github.quazardous.Helios.test-{}", std::process::id());
    let Single::First(first) = instance::single_with(&id, Launch::default()).unwrap() else {
        panic!("nothing else runs under this id");
    };
    let opens = Opens::default();
    opens.listen();
    let ctx = egui::Context::default();
    let mut app = App::builder()
        .plugin(InstancePlugin::new(first).scheme(SCHEME))
        .plugin(Platform::new(opens.clone()))
        .build(&ctx)
        .unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["second_launch_child", "--exact", "--nocapture"])
        .env(CHILD, &id)
        .status()
        .unwrap();
    assert!(status.success(), "the second launch handed over");
    // The launch is delivered before the second one is answered; the
    // window takes it at its next passes.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut got = Vec::new();
    while got.len() < 2 && Instant::now() < deadline {
        app.logic(&ctx);
        got.extend(opens.take());
        std::thread::sleep(Duration::from_millis(10));
    }
    got.sort_by_key(|o| format!("{o:?}"));
    assert_eq!(
        got,
        vec![
            Open::Image("/shoot/photo.jpg".into()),
            Open::Image("/shoot/raw one.dng".into()),
        ]
    );
    let steps: Vec<String> = app
        .handle()
        .step_traces()
        .into_iter()
        .map(|t| t.step.as_str().to_string())
        .collect();
    assert!(
        steps.iter().any(|s| s == "instance.second_launch"),
        "{steps:?}"
    );
    assert!(steps.iter().any(|s| s == "deeplink.open"), "{steps:?}");
}

#[test]
fn a_link_that_opens_nothing_opens_nothing() {
    let opens = Opens::default();
    opens.listen();
    let ctx = egui::Context::default();
    let mut app = App::builder()
        .plugin(
            InstancePlugin::new(instance::Instance::detached(Launch {
                args: vec!["helios://delete?path=/a.jpg".into()],
                ..Launch::default()
            }))
            .scheme(SCHEME),
        )
        .plugin(Platform::new(opens.clone()))
        .build(&ctx)
        .unwrap();
    app.logic(&ctx);
    app.logic(&ctx);
    assert!(opens.take().is_empty());
}

fn notifying(desktop: &Simulated) -> (egui::Context, App) {
    let ctx = egui::Context::default();
    let app = App::builder()
        .plugin(NotifyPlugin::new("Helios").backend(desktop.clone()))
        .plugin(Platform::new(Opens::default()))
        .build(&ctx)
        .unwrap();
    (ctx, app)
}

fn export(seconds: f64) -> JobEnd {
    JobEnd {
        job: Job::Export,
        outcome: Ok("12 images".into()),
        seconds,
    }
}

#[test]
fn a_long_job_is_notified_only_while_unfocused_and_wanted() {
    assert!(should_notify(true, false, LONG_JOB_SECONDS));
    assert!(!should_notify(true, true, 60.0));
    assert!(!should_notify(false, false, 60.0));
    assert!(!should_notify(true, false, 1.0));

    let desktop = Simulated::default();
    let (ctx, mut app) = notifying(&desktop);
    let english = |m: helios_core::Message| m.english;
    assert!(jobs_enabled(app.handle()));
    assert!(!job_finished(app.handle(), &export(60.0), true, english));
    assert!(!job_finished(app.handle(), &export(2.0), false, english));
    assert!(job_finished(app.handle(), &export(60.0), false, english));
    let failed = JobEnd {
        job: Job::Batch,
        outcome: Err("disk full".into()),
        seconds: 30.0,
    };
    assert!(job_finished(app.handle(), &failed, false, english));
    // Shown at the next passes, from the notifications' thread.
    let deadline = Instant::now() + Duration::from_secs(5);
    while desktop.shown().len() < 2 && Instant::now() < deadline {
        app.logic(&ctx);
        std::thread::sleep(Duration::from_millis(10));
    }
    let shown: Vec<(String, String)> = desktop
        .shown()
        .into_iter()
        .map(|(_, n)| (n.title, n.body))
        .collect();
    assert_eq!(
        shown,
        vec![
            ("Export finished".to_string(), "12 images".to_string()),
            ("Batch failed".to_string(), "disk full".to_string()),
        ]
    );

    // The setting off: nothing more.
    app.handle()
        .store()
        .lock()
        .unwrap()
        .set(Set::new(
            &format!("settings/{SETTING_JOBS}"),
            Change::To(Value::Bool(false)),
            Origin::User,
        ))
        .unwrap();
    assert!(!jobs_enabled(app.handle()));
    assert!(!job_finished(app.handle(), &export(60.0), false, english));
}

#[test]
fn the_focus_is_any_helios_window_s() {
    let focused = |states: &[Option<bool>]| {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        for (i, s) in states.iter().enumerate() {
            let id = if i == 0 {
                egui::ViewportId::ROOT
            } else {
                egui::ViewportId::from_hash_of(i)
            };
            input.viewports.insert(
                id,
                egui::ViewportInfo {
                    focused: *s,
                    ..Default::default()
                },
            );
        }
        let mut got = None;
        let mut out = ctx.run_ui(input, |ui| got = Some(window_focused(ui.ctx())));
        out.textures_delta.clear();
        got.unwrap()
    };
    assert!(!focused(&[Some(false)]));
    assert!(focused(&[Some(true)]));
    assert!(focused(&[Some(false), Some(true)]));
    // Nothing known: taken as focused, nothing said.
    assert!(focused(&[None]));
}

#[test]
fn the_about_dialog_says_where_logs_and_crash_reports_are() {
    let dir = std::env::temp_dir().join(format!("helios-platform-{}", std::process::id()));
    let ctx = egui::Context::default();
    let app = App::builder()
        .plugin(LogsPlugin::new(STATE_NAME).logs(Logs::new(dir.join("logs"), STATE_NAME)))
        .plugin(
            CrashPlugin::new("Helios", "0")
                .dir(dir.join("crashes"))
                .hook(false),
        )
        .build(&ctx)
        .unwrap();
    let lines = about(app.handle(), |m| m.english);
    assert_eq!(
        lines,
        vec![
            format!(
                "Log file: {}",
                dir.join("logs").join("helios.log").display()
            ),
            format!("Crash reports: {}", dir.join("crashes").display()),
        ]
    );
    // Without the plugins (tests, pictures), nothing to say.
    let bare = App::builder().build(&egui::Context::default()).unwrap();
    assert!(about(bare.handle(), |m| m.english).is_empty());
    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
}
