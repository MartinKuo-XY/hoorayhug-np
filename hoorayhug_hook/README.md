# hoorayhug Hook

[![crates.io](https://img.shields.io/crates/v/hoorayhug_hook.svg)](https://crates.io/crates/hoorayhug_hook)
[![Released API docs](https://docs.rs/hoorayhug_hook/badge.svg)](https://docs.rs/hoorayhug_hook)

HoorayHug's flexible hooks.

## Pre-connect Hook

```c
// Get the required length of first packet.
uint32_t hoorayhug_first_pkt_len();
```

```c
// Get the index of the selected remote peer.
//
// Remote peers are defined in `remote`(default) and `extra_remotes`(extended),
// where there should be at least 1 remote peer whose idx is 0.
//
// idx < 0 means **ban**.
// idx = 0 means **default**.
int32_t hoorayhug_decide_remote_idx(int32_t max_remote_idx, const char *pkt);
```
