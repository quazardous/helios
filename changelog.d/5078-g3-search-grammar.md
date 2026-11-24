kind: added

Smart album rules and the vault's queries share one search grammar, written and read as text: `rating:>=3 label:red label:blue -flag:reject (camera:~Z OR #beach) captured:last:30d`, with `-` to negate, `OR` in parentheses, ranges (`iso:100..800`, `captured:2024-06`), relative dates and aliases (`stars`, `collection`, `date`). A smart album's rules can now test presence, backup state, culling state, keyword branches and words in the name or tags.
