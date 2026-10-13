kind: internal

`cargo xtask affected` maps a change to the crates it touches and their dependents, plus a critical set (smoke render, load and save round trip, survival), and names the box jobs to run; any change to the build, CI or an unknown path selects everything.
