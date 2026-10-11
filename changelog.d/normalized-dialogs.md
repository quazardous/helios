kind: added

Normalized dialogs: confirmations, forms, pickers, menus and notices are
drawn by the shell alone, the same way everywhere from the theme's tokens
(a test fails if a panel draws its own). A destructive confirmation is
named by its verb, in the danger style, with the focus on Cancel; warnings
and errors show as toasts. `dialog.current` and `dialog.answer` let an
agent read and answer any dialog, `dialog.assumeYes` confirms by policy
for scripts. Detached panel windows get Helios's frame.
