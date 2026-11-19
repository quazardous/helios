kind: changed

The closing splash lists every quit step in order and names the one running: the library's node told to stop and the window's disconnection are listeners of `helios.app.quit`, one per frame, and a step past its deadline is cut while the quit goes on. A quit report (finished, late, failed or left steps, possibly lost work) goes on the journal and the next start says what the last quit cut, after qubit-event-bus's `ShutdownReport` (https://github.com/qubit-ltd/rs-event-bus).
