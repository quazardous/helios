kind: internal

CI checks each engine module's GPU output against its CPU path on lavapipe, against per-adapter budgets: a module over its budget, or a GPU kernel that fell back to the CPU, fails the run.
