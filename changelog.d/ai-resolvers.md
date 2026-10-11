kind: added

Decision nodes can be answered by a model: any program reading and
answering JSON, or an OpenAI-compatible or Anthropic endpoint whose key
comes from the environment, applied at once (full-AI) or kept as a
proposal to accept (AI-assisted), never asked twice about the same pixels
and capped by `system.ai.calls`. `helios control decisions` and the
`decision-*` commands and MCP tools let you or an agent ask, review and
answer; `decide_exposure` is a first decision module.
