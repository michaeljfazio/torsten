---
name: stake-address-info credential filtering
description: Fixed query stake-address-info to filter server-side; reward balance already present
type: project
---

`query stake-address-info` was previously sending `send_query(10)` (a bare Shelley tag with no argument), causing the node to return all stake addresses. The client then filtered client-side by credential hex.

**Fix:** `N2CClient::query_stake_address_info(&[u8])` now takes the 28-byte credential hash as an argument and encodes it as `tag(258) Set<Credential>` in the query payload. This filters server-side in `handle_filtered_delegations` (query_handler/stake.rs).

**Why:** Fetching all stake addresses over the socket is O(n) for the entire network's stake set. Server-side filtering returns only the single address in question.

**Reward balance note:** `rewardAccountBalance` was already present in the output — it maps directly to the `rewards_map: Map<Credential, Coin>` in the GetFilteredDelegationsAndRewardAccounts response. No separate `query reward-account-balance` subcommand needed; cardano-cli also uses `stake-address-info` for this.

**Unregistered address behavior:** If an address is not in the rewards map, we output a single JSON entry with `delegation: null` and `rewardAccountBalance: 0`, matching cardano-cli behavior.

**Wire format:** Shelley query tag 10 = GetFilteredDelegationsAndRewardAccounts. Argument is `tag(258) [array(2)[0, hash(28)]]`. Response is `array(2)[delegations_map, rewards_map]`.
