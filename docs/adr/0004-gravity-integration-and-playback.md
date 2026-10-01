# ADR 0004: Gravity, integration and playback

Status: implementation in progress; acceptance evidence remains open.

## Prerequisite gate review — 2026-10-01

The operator explicitly approved proceeding with Phase 3 while retaining the
unverified Phase 2 Windows visual sequence and Phase 1/2 Linux native, interactive
and current-revision remote-CI criteria. This is a reviewed sequencing deferral,
not acceptance of missing evidence. Existing Phase 2 work was preserved in a
separate baseline commit after Windows formatting, workspace tests and Clippy.

## Gravity decision

Use CODATA 2018 G=6.67430e-11, unsoftened inertial SI f64 point masses,
lexicographic dense i<j pairs and ordinary serial component accumulation.
Both members react. Robust hypot distance and checked arithmetic reject
coincidence/unrepresentable forces; physical radius is absent from the kernel.
The chi<=0.02 session envelope uses square-root-scaled pair magnitudes without
forming r cubed or a potentially overflowing sum of masses.

O(N²) forces and O(N) scratch are appropriate for 3–16 bodies. Approximation,
parallel reductions and GPU gravity require a separate measured scope review.
