kind: added

Settings are declared once and resolved through layers (defaults, global,
project, user, run), and every value says which layer it comes from. A
value set where it does not belong, such as a machine setting in a project
file, is refused with the reason.
