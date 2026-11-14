kind: added

AI calls have limits per provider in `ai.toml` (`[limits.providers.<id>]`: calls at once, calls per minute): a call over them waits instead of failing, and a local runtime runs one vision call at a time by default. A run may make at most `limits.run_remote_calls` remote calls (500). Agents check a run's model nodes before it starts with `helios_ai_check`: what nothing can serve, and which images would leave the machine and to whom.
