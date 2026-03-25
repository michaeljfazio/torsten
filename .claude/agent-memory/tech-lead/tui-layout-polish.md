---
name: TUI polish feature/tui-polish
description: Major TUI UX overhaul — layout, alignment, Monokai default, RTT approximation
type: project
---

TUI layout and UX overhauled on branch `feature/tui-polish`.

**Key changes:**
- `layout.rs`: Added `LayoutMode::Wide` (>= 120 x 30) in addition to Standard/Compact. Header now 2 lines (HEADER_H=2). Panel heights tuned for more content.
- `ui.rs`: Full rewrite. Label-left / value-right alignment via `kv_aligned(label, value, color, theme, col_w)`. Status pill in header uses colored background. RTT bar is colored per-band (success/info/warning/error). Footer has themed pill showing current theme name. No duplicated info across panels.
- `app.rs`: Default theme is Monokai (index 1, found by name search). RTT min/max approximated from lowest/highest populated bucket midpoints using `.or()` instead of `.or_else(|| ...)` to satisfy clippy.
- `epoch_progress.rs`: Removed `#[allow(dead_code)]` from `with_fill_color` (now actively called from both header and chain panel).

**Panel information map (no duplicates):**
- Node: Role, Network, Version, Era, Uptime, Forged blocks, UTxO count
- Chain: epoch bar, Block, Slot, Slot/Epoch, Tip diff, Density, Forks, Total Tx, Pending Tx
- Connections: P2P, Inbound, Outbound, Cold/Warm/Hot, Duplex
- Resources: CPU %, Mem live, Mem RSS, inline mini-bars
- Peers: RTT bar, band counts (0-50ms/50-100ms/100-200ms/200ms+), Low/Avg/High

**Why:** Performance, readability, and professional appearance matching htop/lazydocker aesthetic.

**How to apply:** When extending the TUI, follow the kv_aligned pattern (impl Into<String> for value), keep information in its home panel, and use theme colors rather than hardcoded Color values.
