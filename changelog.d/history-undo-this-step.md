kind: added

The History panel's "Undo this step" takes back one step of an image's
edit alone, even under later steps, as long as none of them changed the
same values; it is then on the redo list. Agents reach it as
`history.undo_step`.
