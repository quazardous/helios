kind: changed

The Library in its own window no longer redraws on every pass of the main window: it draws when something came in for it (thumbnails, renders, marks, a read answered). The Library and import windows ask the main window's pass on the bus (`window.main_pass`) instead of repainting it by hand.
