kind: fixed

Connecting or reconnecting an account (Google Photos) and "Pick photos…" open the browser at once: the page is opened as soon as it is ready, through the system's own opener (the desktop portal inside the Flatpak), instead of waiting for the import window's next frame. The dialog shows the page's address, to copy, with "Open again". Every "Pick photos…" starts a new picking session; the dialog says which account the browser must be signed in to, and a session that expires is reported ("The picking session expired, start again") with "Start again", instead of being waited out.
