kind: internal

Helios builds on egui-pop 0.18.3 (core and viewport 0.15.1) and on egui_hud 0.10.0 from crates.io, no longer on a git revision of egui_hud, so `next` holds no git pin a release would refuse.

The Parameters panel and the controls gallery write the values egui_hud 0.10.0 reports from egui-pop's editors (a curve's points, a wheel's colour) at the end of their gesture, as other set values are; a drag given up writes nothing.
