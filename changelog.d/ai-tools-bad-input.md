kind: fixed

The AI tools no longer crash the node on bad input: `helios_ai_prompts_set` refuses a `prompt` that is not an object, and `helios_ai_local_pull` refuses a `size_mib` above 4 TiB, each with a clear error.
