# AI models: providers, routing, prompts, local setup

Status: **first step** (2026-10-13). Decided by the owner on that date:

- **No built-in assistant in Helios.** MCP is the only control surface: an
  external agent (the user's own) sets up and tunes everything described
  here, through the `helios_ai_*` tools. Helios ships no chat window.
- **Local-first.** A photographer with a strong machine wants a local model
  for some tasks; the agent installs and configures it for them. A remote
  provider exists only once someone adds it.

The code lives in `helios-ai` (providers, routing, prompts, the local
runtime, the call log); `helios-resolve` routes decision nodes through it;
`helios-mcp` exposes it; `helios doctor ai` reports on it.

## Providers and capabilities

A provider is something that answers model calls. Each declares what it can
do, as **capabilities**:

| Capability | What it is | Used by |
|---|---|---|
| `chat` | text in, text out | descriptions, template drafts |
| `vision` | text and images in, text out | decision nodes, image prompts |
| `embeddings` | text in, vectors out | similarity, search |
| `structured` | an answer that fits a JSON schema | every prompt with an output schema |
| `segmentation` | an image and a prompt in, a mask out | AI object masks |

Provider kinds:

| Kind | Talks to | Capabilities | Local? |
|---|---|---|---|
| `ollama` | Ollama's API (`/api/chat`, `/api/embed`, `/api/tags`, `/api/pull`) | chat, vision, embeddings, structured | yes, on loopback |
| `openai` | any OpenAI-compatible server (`/v1/chat/completions`, `/v1/models`, `/v1/embeddings`): llama.cpp's server, LM Studio, `vLLM`, OpenAI | chat, vision, embeddings, structured | when its URL is loopback |
| `anthropic` | Anthropic's messages API | chat, vision, structured | no |
| `command` | a local program reading JSON on stdin | segmentation | yes |

A provider entry may narrow its capabilities (`capabilities = [...]`): an
OpenAI-compatible server running a text-only model does not do vision.
Model-level capabilities come from the runtime when it says (Ollama's
`/api/show`), else from the model's name, and are listed by
`helios_ai_models_list`.

The decision resolvers that existed before (`helios-resolve`'s `Http`, with
an OpenAI-compatible or Anthropic endpoint) now call these same providers:
one code path for the wire formats.

## Routing

The routing map says which provider and model answer each capability, and
optionally each prompt:

```toml
# ai.toml
[routing.capabilities]
vision = { provider = "ollama", model = "qwen2.5vl:7b" }
embeddings = { provider = "ollama", model = "nomic-embed-text" }

[routing.prompts]
"image.describe" = { provider = "ollama", model = "gemma3:4b" }
```

A call for a prompt takes the prompt's route first, then its capability's.
`structured` falls back to `chat` and the reverse. Nothing is routed by
default: an unrouted capability is unavailable, and `helios doctor ai` says
so and why. `helios_ai_local_bench` fills the map from measurements when
asked to (`apply`); `helios_ai_routing_set` sets one route by hand.

Decision nodes go through `helios-resolve`'s `Routed`: a decision naming a
resolver of the network uses it as before; once `Routed` is given the AI
service, one naming a capability (`"resolver": "segmentation"`), or naming
none when the network has no default, goes through the routing map
(`vision` for a decision, and `segmentation` for a mask). A network opts
in with `"resolver": "routing"`.

## Prompts as versioned assets

A prompt is a file: its id, its version, its capability, the model it was
tuned for, its text, and the schemas of its input and its output.

```toml
id = "image.describe"
version = 1
capability = "vision"
description = "A short caption and keywords for one photograph"
tuned_for = ["qwen2.5vl:7b"]
images = 1
system = "You describe photographs for a photographer's catalogue."
template = "Describe this photograph. Language: {{language}}."

[input]
type = "object"
required = ["language"]
properties.language = { type = "string" }

[output]
type = "object"
required = ["caption", "keywords"]
# ...

[example]
language = "English"
```

- **Shipped prompts** are files in `crates/helios-ai/prompts/`, built into
  Helios. They never change once released: a better one is a new version.
- **User prompts** live in the AI directory (`prompts/`, one JSON file per
  version). `helios_ai_prompts_set` always writes a new version (the next
  number for that id): a version, once written, never changes.
- **Pinning.** A workflow names a prompt as `id` (the latest version) or
  `id@version` (that version, always). Reproducible work pins.
- **Validation.** The input is checked against the input schema before the
  call, the reply against the output schema after it. The schemas use a
  subset of JSON Schema: `type`, `properties`, `required`,
  `additionalProperties`, `items`, `enum`, `minimum`, `maximum`,
  `minLength`, `maxLength`, `minItems`, `maxItems`.
- **Trying.** `helios_ai_prompts_try` runs a prompt on the current image (a
  node's output, exported) or on a file, and returns the raw reply and the
  validated answer, with the errors if it does not validate. It applies
  nothing.

Hooks for features: `Ai::run_prompt` runs a prompt by reference and returns
the validated answer or why not. Intake's stack suggestions
(`stacks.suggest`) and template design (`template.design`) use it with
their own prompts: they add a prompt file, and the routing map, the call
log and the bench then cover them with no other change.

## Local setup

The agent sets up a local runtime (Ollama, the first one supported) for the
photographer, in steps it can inspect:

1. `helios_ai_local_probe`: the GPU (vendor, name, VRAM; unified memory on
   Apple silicon), RAM, CPU, the free disk where models go, the OS, and the
   memory budget a model may take. Read from `nvidia-smi`, `/sys/class/drm`,
   `/proc`, `sysctl` and `df`, with no new dependency.
2. `helios_ai_local_status`: whether the runtime is installed (on `PATH`),
   running (it answers on its URL), its version and its models.
3. `helios_ai_local_recommend`: per capability, the models of a small
   built-in catalogue that fit the budget, best first, with their size. The
   sizes are approximate; the runtime has the last word when it pulls.
4. `helios_ai_local_setup`: the exact steps for this OS. On Linux, a
   per-user install (the release archive unpacked under `~/.local`, a
   systemd user service), never the system-wide script, which needs root;
   on macOS, Homebrew; on Windows, winget. **Nothing runs unless the call
   passes `confirm` with that step's id**, and a step needing root is only
   ever shown, never run. Helios refuses to run any step when it runs as
   root itself.
5. `helios_ai_local_pull`: downloads a model as a job, after checking the
   disk has room for it, with `job.progress` events on the AI bus;
   `helios_ai_jobs_cancel` stops it.
6. `helios_ai_local_bench`: runs every shipped prompt (or the ones named)
   on every local model able to, measures the latency, validates the
   answers, and with `apply` routes each prompt and capability to the
   fastest model that answered correctly. It runs as a job too.

## Privacy

- **Local-first by default.** With no provider configured, Helios knows one
  provider: Ollama on loopback. Nothing is remote until someone adds it.
- **Remote only when added**, by `helios_ai_providers_add`. Every provider
  is listed with `remote` true or false (a loopback URL is local).
- **Keys.** A key is never written to a Helios file. A provider names where
  its key is: an environment variable (`key = { env = "OPENAI_API_KEY" }`),
  or the system keyring (`key = { keyring = "openai" }`), filled by
  `helios_ai_providers_add` with `api_key`. The keyring is reached through
  the system's own tool: `secret-tool` (the Secret Service, GNOME Keyring or
  KWallet) on Linux, `security` (the login keychain) on macOS. This avoids
  a native D-Bus dependency; Windows has no keyring support yet and uses an
  environment variable. The environment variable is preferred when the agent
  should never see the key.
- **Images are downscaled** before they leave the process: to
  `privacy.remote_image_side` (1024 px by default) for a remote provider,
  `privacy.local_image_side` (2048 px) for a local one, and re-encoded,
  which drops their metadata (GPS position included).
- **Every call is logged and published as a job.** Each call runs as a
  scheduler job (`scheduler.job_*` events, limited by `limits.calls`), is
  published as an `ai.call` event, and is appended to `calls.jsonl`: when,
  the provider, the model, the prompt and its content hash, local or remote,
  how many images and bytes left, how long, and the error if any. Never the
  images or the key.

## Reproducibility

A model answer is not reproducible by nature; Helios makes it traceable and
as stable as the runtime allows:

- a decision node's answer is recorded in the edit (see
  [module-contract.md](module-contract.md)), with the model's name and
  version: renders replay it, nothing is asked again;
- a prompt is pinned by `id@version`, and its content hash goes in the call
  log, so a changed prompt is never mistaken for the old one;
- prompts declare their `temperature` (0 by default) and a `seed`, passed
  to runtimes that take one (Ollama, OpenAI-compatible servers);
- the model runtime's own version is in `helios_ai_local_status`.

## Files

| What | Where |
|---|---|
| Providers, routing, privacy, limits | `ai.toml` in the AI directory |
| User prompts | `prompts/<id>@<version>.json` in the AI directory |
| Call log | `calls.jsonl` in the AI state directory |

The AI directory is `$HELIOS_AI_DIR` when set (a sandbox), else
`$XDG_CONFIG_HOME/helios/ai` (`~/.config/helios/ai`); the state directory
is `$HELIOS_AI_DIR` again, else `$XDG_STATE_HOME/helios/ai`
(`~/.local/state/helios/ai`). Providers and routing describe this machine
(a local runtime): they are not part of a profile yet.

## MCP tools

The owner's dotted names are the requests' `cmd`; MCP tool names use
underscores, like every Helios tool, since several clients (Anthropic's API
among them) accept only `[a-zA-Z0-9_-]` in a tool name.

| Tool | Request | What it does |
|---|---|---|
| `helios_ai_providers_list` | `ai.providers.list` | providers, kinds, capabilities, local or remote, where the key is |
| `helios_ai_providers_add` | `ai.providers.add` | adds or replaces a provider; `api_key` goes to the keyring |
| `helios_ai_providers_remove` | `ai.providers.remove` | removes one, and the routes to it |
| `helios_ai_providers_test` | `ai.providers.test` | reachability, models, a short call to a model |
| `helios_ai_models_list` | `ai.models.list` | each provider's models and their capabilities |
| `helios_ai_routing_get` | `ai.routing.get` | the routing map, each route checked |
| `helios_ai_routing_set` | `ai.routing.set` | sets or clears a capability's or a prompt's route |
| `helios_ai_prompts_list` | `ai.prompts.list` | every prompt and its versions |
| `helios_ai_prompts_get` | `ai.prompts.get` | one prompt version, whole |
| `helios_ai_prompts_set` | `ai.prompts.set` | writes a new user version |
| `helios_ai_prompts_try` | `ai.prompts.try` | runs a prompt, returns raw and validated, applies nothing |
| `helios_ai_local_probe` | `ai.local.probe` | GPU, VRAM, RAM, CPU, disk, OS, budget, what fits |
| `helios_ai_local_status` | `ai.local.status` | runtime installed, running, where, which models |
| `helios_ai_local_recommend` | `ai.local.recommend` | models per capability fitted to the probe |
| `helios_ai_local_pull` | `ai.local.pull` | pulls a model as a job (disk checked first) |
| `helios_ai_local_bench` | `ai.local.bench` | measures prompts on local models as a job; `apply` routes |
| `helios_ai_local_setup` | `ai.local.setup` | the install steps; runs only the confirmed ones |
| `helios_ai_jobs_events` | `ai.jobs.events` | AI bus events after a sequence number |
| `helios_ai_jobs_wait` | `ai.jobs.wait` | waits for a job, returns its state and result |
| `helios_ai_jobs_cancel` | `ai.jobs.cancel` | cancels a pull or a bench |
| `helios_ai_doctor` | `ai.doctor` | the same report as `helios doctor ai` |

These tools run in the MCP server itself: they need no node, except
`helios_ai_prompts_try` on the current image, which exports a node's
output through the control socket first.

## Not yet

- A provider for llama.cpp's own runtime management (it is reached as an
  OpenAI-compatible server, but not installed or pulled by Helios).
- Providers and routing in a profile, for a user's several machines.
- Calls made by a bench are logged and published one by one, but run
  inside the bench's own job rather than as a job each.
- The desktop's own decision resolution does not use the routing map yet;
  a network opts in with `"resolver": "routing"`.
- A keyring on Windows.
- Segmentation by a runtime (no local runtime serves a segmentation model
  over a standard API yet): a `command` provider wraps one.
