# Generated from Helios's declarations and code by `cargo xtask i18n extract`:
# do not edit. Translations live in i18n/<locale>/ under the same names.

category-decisions = Decisions

category-developer = Developer

category-masks = Masks

category-node-graph = Node graph

category-preferences = Preferences

category-vault = Vault

category-workspace = Workspace

command-graph-edit =
    .description = Asks a node editor for an edit, done at its next frame as the user's (a connection type-checked, then its graph.* step): `edit` is `connect` (`from`, `to`), `disconnect` (`edges`), `move` (`moves`), `delete` (`nodes`, `edges`) or `add` (`nodes`).

command-graph-export =
    .description = A node editor's graph as an image, drawn from the graph and its layout as the editor drew it last: `format` (svg by default; png or pdf when built with them), `selection` (true: only the selected nodes), `level` (overview, ports or detail; ports by default), `wires` (curves, orthogonal or straight; the editor's by default), `background` (false: transparent), `title`, `scale` (a PNG's pixels per point, 1 by default). Gives `svg` as text, or `png` or `pdf` base64-encoded.

command-graph-read =
    .description = The graph a node editor drew last: nodes (place, ports, group, parameters), wires, groups, the kinds it can add, the selection, the level and the part in view.

command-graph-select =
    .description = Selects nodes of a node editor (`nodes`, their ids) at its next frame.

command-graph-view =
    .description = Fits a node editor's graph (`fit`: true), zooms to a level (`level`: overview, ports or detail) or focuses a node (`focus`), at its next frame.

frame-helios-compare =
    .title = Compare

frame-helios-culling =
    .title = Culling

frame-helios-darkroom =
    .title = Darkroom

frame-helios-library =
    .title = Library

frame-helios-mask-editing =
    .title = Mask editing

frame-helios-workspace =
    .title = Network

menu-helios-masks-row-add =
    .label = Add

menu-helios-system-profiles =
    .label = Profiles

menu-helios-system-recent =
    .label = Recent workspaces

menu-helios-system-themes =
    .label = Themes

plugin-egui-pop-imaging =
    .title = Image tools

plugin-helios-clipboard =
    .title = Helios's clipboard types

plugin-helios-commands =
    .title = Helios

plugin-helios-contexts =
    .title = Helios places and modes

plugin-helios-display =
    .title = Display

plugin-helios-frame =
    .title = Helios frame

plugin-helios-history =
    .title = Helios history

plugin-helios-lists =
    .title = Helios lists

plugin-helios-platform =
    .title = Helios platform

plugin-helios-theme-editor =
    .title = Theme editor

plugin-helios-workbench =
    .title = Helios areas

setting-display-wide-gamut-preview =
    .description = Shows the image in Rec.2020 where the window can carry it (Wayland with colour management), so colours outside sRGB are not clipped. Elsewhere the image stays sRGB.
    .label = Wide-gamut preview

setting-notifications-jobs =
    .description = Shows a desktop notification when a long export or batch finishes or fails while no Helios window has the focus.
    .label = Notify when a long job ends
