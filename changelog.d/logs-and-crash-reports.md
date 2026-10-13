kind: added

Helios now keeps a log file and writes a crash report when it fails, both
in your state directory (`~/.local/state/helios` on Linux). Nothing is
sent anywhere: after a crash, the next start offers the report, and
Help > About Helios says where both are.
