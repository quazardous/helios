kind: added

Plugins can declare their own health checks in their manifest, through the same contract as Helios's modules: Help > Health check runs them through the plugin, with a time limit, and a check whose plugin crashes or does not answer in time shows as that check's problem. The Google Photos plugin uses it to say which account needs reconnecting, with a Reconnect button.
