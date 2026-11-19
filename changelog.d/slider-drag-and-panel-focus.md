kind: fixed

A Parameters slider keeps its drag once its value changes: the drag goes on, ends as one undo step, and Escape puts the value back (egui-pop-core 0.13.3). Focusing a panel no longer focuses its first slider, so the wheel over an unfocused slider scrolls the panel; Tab enters the panel's widgets (egui-pop-api 0.13.1).
