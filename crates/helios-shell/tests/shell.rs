//! The shell's logic, headless: commands.

use helios_shell::commands::{CommandSpec, Origin, Registry};
use helios_shell::when::Context;
use serde_json::{Value, json};

#[test]
fn commands_run_by_id_from_any_origin_and_are_recorded() {
    let mut r = Registry::new();
    r.register(
        CommandSpec::new("culling.rate", "Rate", "Culling").when("gridFocus"),
        |args, _| Ok(args["stars"].clone()),
    )
    .unwrap();
    assert!(
        r.register(CommandSpec::new("culling.rate", "Again", "x"), |_, _| Ok(
            Value::Null
        ))
        .is_err()
    );
    let grid = Context::new().flag("gridFocus", true);
    assert_eq!(
        r.run("culling.rate", json!({"stars": 3}), Origin::User, &grid),
        Ok(json!(3))
    );
    let agent = Origin::Agent("session-1".into());
    let err = r
        .run("culling.rate", json!({}), agent.clone(), &Context::new())
        .unwrap_err();
    assert!(err.contains("does not apply"), "{err}");
    assert!(r.run("nope", Value::Null, agent.clone(), &grid).is_err());
    assert_eq!(r.history().len(), 2, "unknown commands are not runs");
    assert_eq!(r.history()[1].origin, agent);
    assert_eq!(r.search("rate cull", &grid).len(), 1);
    assert!(
        r.search("rate", &Context::new()).is_empty(),
        "not offered where it does not apply"
    );
}
