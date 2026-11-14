//! The routing map: which provider and model answer a capability, and
//! optionally one prompt.
//!
//! ```toml
//! [routing.capabilities]
//! vision = { provider = "ollama", model = "qwen2.5vl:7b" }
//!
//! [routing.prompts]
//! "image.describe" = { provider = "ollama", model = "gemma3:4b" }
//! ```
//!
//! A call for a prompt takes the prompt's route first, then its
//! capability's; `structured` and `chat` stand in for each other. Nothing
//! else is guessed: an unrouted capability is unavailable.
//!
//! A call may also carry [`Overrides`]: a provider and/or a model given
//! by the run, the node, or the diagram it belongs to, tried in that
//! order before the map ([`Routing::resolve`]).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Capability;

/// A provider and one of its models.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Route {
    pub provider: String,
    pub model: String,
}

impl Route {
    pub fn new(provider: &str, model: &str) -> Self {
        Route {
            provider: provider.into(),
            model: model.into(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Routing {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub capabilities: BTreeMap<Capability, Route>,
    /// By prompt id (any version).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub prompts: BTreeMap<String, Route>,
}

/// Which entry of the map (or which override) a route came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteFrom {
    Prompt(String),
    Capability(Capability),
    /// The highest level of [`Overrides`] that gave the provider or the
    /// model.
    Override(Level),
}

/// Where an override comes from, highest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// One run of a diagram (or one call).
    Run,
    /// One node (or one extractor use on it).
    Node,
    /// The whole diagram.
    Diagram,
}

/// A provider and/or a model to use instead of the routing map's.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl RouteOverride {
    /// A provider and its model.
    pub fn route(provider: &str, model: &str) -> Self {
        RouteOverride {
            provider: Some(provider.into()),
            model: Some(model.into()),
        }
    }

    /// Another model of whichever provider is chosen.
    pub fn model(model: &str) -> Self {
        RouteOverride {
            provider: None,
            model: Some(model.into()),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.provider.is_none() && self.model.is_none()
    }
}

/// The overrides a call carries, by level. Each field is taken from the
/// highest level that gives it: run, then node, then diagram, then the
/// routing map. A model given below the level that chose the provider
/// is ignored (it was meant for another provider).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Overrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<RouteOverride>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<RouteOverride>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagram: Option<RouteOverride>,
}

impl Overrides {
    /// The levels in resolution order.
    pub fn levels(&self) -> [(Level, Option<&RouteOverride>); 3] {
        [
            (Level::Run, self.run.as_ref()),
            (Level::Node, self.node.as_ref()),
            (Level::Diagram, self.diagram.as_ref()),
        ]
    }

    pub fn is_empty(&self) -> bool {
        self.levels()
            .iter()
            .all(|(_, o)| o.is_none_or(RouteOverride::is_empty))
    }
}

impl Routing {
    /// The route for `capability`, or for the prompt `prompt` first.
    pub fn route(
        &self,
        capability: Capability,
        prompt: Option<&str>,
    ) -> Option<(Route, RouteFrom)> {
        if let Some(id) = prompt
            && let Some(r) = self.prompts.get(id)
        {
            return Some((r.clone(), RouteFrom::Prompt(id.to_string())));
        }
        if let Some(r) = self.capabilities.get(&capability) {
            return Some((r.clone(), RouteFrom::Capability(capability)));
        }
        let stand_in = match capability {
            Capability::Structured => Capability::Chat,
            Capability::Chat => Capability::Structured,
            _ => return None,
        };
        self.capabilities
            .get(&stand_in)
            .map(|r| (r.clone(), RouteFrom::Capability(stand_in)))
    }

    /// The route for `capability` (or the prompt `prompt`) once
    /// `overrides` are applied, or why there is none.
    pub fn resolve(
        &self,
        capability: Capability,
        prompt: Option<&str>,
        overrides: &Overrides,
    ) -> Result<(Route, RouteFrom), String> {
        let base = self.route(capability, prompt);
        let levels = overrides.levels();
        // Index 3 stands for the map.
        let chosen = levels
            .iter()
            .enumerate()
            .find_map(|(i, (_, o))| o.and_then(|o| o.provider.clone()).map(|p| (i, p)));
        let (provider_at, provider) = match (chosen, &base) {
            (Some((i, p)), _) => (i, p),
            (None, Some((r, _))) => (3, r.provider.clone()),
            (None, None) => {
                return Err(
                    match levels.iter().find_map(|(l, o)| {
                        o.and_then(|o| o.model.as_ref()).map(|m| (*l, m.clone()))
                    }) {
                        Some((level, model)) => format!(
                            "the {} override names model `{model}` but no provider, and nothing \
                         is routed for {capability}{}",
                            level_name(level),
                            prompt.map(|p| format!(" or `{p}`")).unwrap_or_default()
                        ),
                        None => format!(
                            "nothing is routed for {capability}{}",
                            prompt.map(|p| format!(" or `{p}`")).unwrap_or_default()
                        ),
                    },
                );
            }
        };
        let model = levels
            .iter()
            .enumerate()
            .take(provider_at + 1)
            .find_map(|(i, (_, o))| o.and_then(|o| o.model.clone()).map(|m| (i, m)));
        let (model_at, model) = match (model, &base) {
            (Some(m), _) => m,
            (None, Some((r, _))) if r.provider == provider => (3, r.model.clone()),
            (None, _) => {
                return Err(format!(
                    "the {} override names provider `{provider}` but no model",
                    level_name(levels[provider_at].0)
                ));
            }
        };
        let top = provider_at.min(model_at);
        let from = match (top, base) {
            (3, Some((_, from))) => from,
            (i, _) => RouteFrom::Override(levels[i.min(2)].0),
        };
        Ok((Route { provider, model }, from))
    }

    /// Removes every route to `provider`; says which.
    pub fn forget(&mut self, provider: &str) -> Vec<String> {
        let mut removed = Vec::new();
        self.capabilities.retain(|c, r| {
            let keep = r.provider != provider;
            if !keep {
                removed.push(c.name().to_string());
            }
            keep
        });
        self.prompts.retain(|p, r| {
            let keep = r.provider != provider;
            if !keep {
                removed.push(p.clone());
            }
            keep
        });
        removed
    }
}

fn level_name(level: Level) -> &'static str {
    match level {
        Level::Run => "run",
        Level::Node => "node",
        Level::Diagram => "diagram",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prompt_route_wins_then_the_capability_then_its_stand_in() {
        let mut routing = Routing::default();
        routing
            .capabilities
            .insert(Capability::Vision, Route::new("ollama", "big"));
        routing
            .capabilities
            .insert(Capability::Chat, Route::new("ollama", "chat"));
        routing
            .prompts
            .insert("image.describe".into(), Route::new("lan", "small"));
        assert_eq!(
            routing.route(Capability::Vision, Some("image.describe")),
            Some((
                Route::new("lan", "small"),
                RouteFrom::Prompt("image.describe".into())
            ))
        );
        assert_eq!(
            routing
                .route(Capability::Vision, Some("image.assess"))
                .unwrap()
                .0,
            Route::new("ollama", "big")
        );
        assert_eq!(
            routing.route(Capability::Structured, None).unwrap().1,
            RouteFrom::Capability(Capability::Chat)
        );
        assert_eq!(routing.route(Capability::Embeddings, None), None);
        assert_eq!(routing.forget("lan"), vec!["image.describe".to_string()]);
    }

    #[test]
    fn overrides_resolve_run_then_node_then_diagram_then_the_map() {
        let mut routing = Routing::default();
        routing
            .capabilities
            .insert(Capability::Vision, Route::new("ollama", "map"));
        let vision = |o: &Overrides| routing.resolve(Capability::Vision, Some("x.y"), o);
        let mut o = Overrides::default();
        assert_eq!(
            vision(&o).unwrap(),
            (
                Route::new("ollama", "map"),
                RouteFrom::Capability(Capability::Vision)
            )
        );
        o.diagram = Some(RouteOverride::route("lan", "diagram"));
        assert_eq!(
            vision(&o).unwrap(),
            (
                Route::new("lan", "diagram"),
                RouteFrom::Override(Level::Diagram)
            )
        );
        o.node = Some(RouteOverride::route("cloud", "node"));
        assert_eq!(
            vision(&o).unwrap(),
            (
                Route::new("cloud", "node"),
                RouteFrom::Override(Level::Node)
            )
        );
        o.run = Some(RouteOverride::route("ollama", "run"));
        assert_eq!(
            vision(&o).unwrap(),
            (Route::new("ollama", "run"), RouteFrom::Override(Level::Run))
        );
        // A model alone keeps the provider chosen below it.
        o.run = Some(RouteOverride::model("other"));
        assert_eq!(
            vision(&o).unwrap(),
            (
                Route::new("cloud", "other"),
                RouteFrom::Override(Level::Run)
            )
        );
        // A provider alone takes no model meant for another provider.
        o.run = Some(RouteOverride {
            provider: Some("lan".into()),
            model: None,
        });
        assert!(
            vision(&o)
                .unwrap_err()
                .contains("run override names provider `lan` but no model")
        );
        // ... but the map's model when the map names that provider.
        o = Overrides {
            node: Some(RouteOverride {
                provider: Some("ollama".into()),
                model: None,
            }),
            ..Overrides::default()
        };
        assert_eq!(
            vision(&o).unwrap(),
            (
                Route::new("ollama", "map"),
                RouteFrom::Override(Level::Node)
            )
        );
        // Nothing routed: a whole override still serves, a model alone not.
        let empty = Routing::default();
        let o = Overrides {
            run: Some(RouteOverride::route("lan", "m")),
            ..Overrides::default()
        };
        assert_eq!(
            empty.resolve(Capability::Chat, None, &o).unwrap().0,
            Route::new("lan", "m")
        );
        let o = Overrides {
            run: Some(RouteOverride::model("m")),
            ..Overrides::default()
        };
        assert!(
            empty
                .resolve(Capability::Chat, None, &o)
                .unwrap_err()
                .contains("no provider")
        );
        assert!(o.levels()[0].1.is_some() && !o.is_empty());
        assert!(Overrides::default().is_empty());
    }

    #[test]
    fn the_map_reads_as_toml() {
        let routing: Routing = toml::from_str(
            "[capabilities]\nvision = { provider = \"ollama\", model = \"qwen2.5vl:7b\" }\n\
             [prompts]\n\"image.describe\" = { provider = \"ollama\", model = \"gemma3:4b\" }\n",
        )
        .unwrap();
        assert_eq!(
            routing.capabilities[&Capability::Vision],
            Route::new("ollama", "qwen2.5vl:7b")
        );
        assert_eq!(routing.prompts.len(), 1);
    }
}
