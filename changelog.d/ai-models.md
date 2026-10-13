kind: added

AI model providers with declared capabilities (Ollama, any OpenAI-compatible
server, Anthropic, a local command), a routing map from capability or prompt
to provider and model, and prompts as versioned files that a workflow can
pin. Local-first: only Ollama on loopback is known until a provider is added;
keys stay in the system keyring or the environment, images are downscaled
before they leave, and every call is logged.
