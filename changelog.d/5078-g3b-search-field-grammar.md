kind: changed

Library: the search field reads its whole text with the vault's search grammar: `-flag:reject`, `(label:red OR #beach)`, `iso:<=800`, `captured:last:30d`, `+label:red +label:blue` and the rest work as typed, and `library.filter.apply` takes the same text. A workspace's library query keeps any such terms too (`search`), not only stars, labels, flags and tags.
