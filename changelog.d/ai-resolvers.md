kind: added

Decision nodes can now be answered by a model. Helios asks for every
decision waiting on one, at most as many at once as the `ai_calls` limit
allows, shows the model a preview of what the node looks at, and never
asks twice about the same pixels. In full-AI mode the answer is applied
and the render goes on; in AI-assisted mode it waits as a proposal for
you to accept or correct. A model can be any program that reads the
question as JSON and answers in JSON, so a local model or your own
script around an online service works without Helios holding any key.

Helios can also call a vision model directly: any OpenAI-compatible
endpoint (OpenAI, or a local server speaking its protocol) or Anthropic's,
with the key read from an environment variable, never from a file.
