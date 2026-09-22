# size-bench provenance

Hand-written microbenches for the XC8 size-parity track, one file per
`density-profile.py` sink. Each bench sinks its result into a `volatile`
global so the exercised operation cannot be folded away. Size-only pins;
behavioral correctness of the ops themselves is covered by the existing
e2e suite.
