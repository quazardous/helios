kind: fixed

A monitor unplugged while no Helios window moves or changes scale is now noticed within a few seconds (Helios re-reads the monitors every 3 seconds while one of its windows has the focus or more than one screen is connected), so a window left on it is moved back without waiting for an event. An idle Helios still repaints nothing.
