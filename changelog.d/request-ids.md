kind: added

A change sent to a node may carry a request id (`_meta.request_id`). Sent again with the same id, as after an "outcome unknown", the node applies it once and answers the first result marked `already_applied`. The MCP tools that change something and `helios run` / `helios control` send one (`request_id`, `--request-id`), named after an outcome unknown. A node keeps up to 1,000 ids, for 10 minutes each.
