//! The desktop's platform services: a second launch, another process,
//! hands its files and links to the running Helios.

use std::process::Command;
use std::time::{Duration, Instant};

use egui_pop::api::App;
use egui_pop::platform::instance::{self, InstancePlugin, Launch, Single};
use helios_gui::platform::{Open, Opens, Platform, SCHEME};

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
    assert!(steps.iter().any(|s| s == "instance.second_launch"), "{steps:?}");
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
