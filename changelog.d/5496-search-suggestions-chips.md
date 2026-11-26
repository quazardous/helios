kind: added

Library: the search field suggests the keys the word typed starts (`ca` offers "Camera — camera:"; Down and Enter, Tab or a click takes one), and each key term typed in full (a space after it, or Enter) becomes a chip in words ("camera is R5"). Backspace in the empty field selects the last chip, then removes it; Ctrl+Backspace removes it at once; Escape closes the suggestions, clears the text, then goes back to the grid. The chips' rules are part of the filter `library.filter` reports and applies.
