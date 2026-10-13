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

/// Which entry of the map a route came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteFrom {
    Prompt(String),
    Capability(Capability),
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
