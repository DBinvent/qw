# QW knownby.work — Skills confirmed by the people you worked with. Found through friends of friends.

A referral-network protocol for trust-based work exchange, built on
Nostr. See `todo-impl.md` for the implementation plan and current status.

**Status:** early prototype (protocol layer + a local referral-routing
demo). Nothing here is ready for real transactions or real personal data.

## Legal notices

Read these before publishing anything through this protocol, and before
referencing this project in any external pitch, investor conversation, or
public description. Neither notice is legal advice.

### Co-authorship, not barter

This protocol's co-authorship framing is not a general exemption from
barter/service taxation. It holds only when work is on a declared
open-source project (the output is public and non-appropriable) and no
project involved is controlled by the counterparty in a way that
privatizes the benefit. Direct bilateral work-for-work, or contribution
to a counterparty-controlled private project, falls outside this framing
and is the participants' own tax responsibility to assess — this
protocol does not determine or launder that responsibility. This framing
has not been confirmed by a written tax attorney opinion; treat it as an
engineering description of the system's structure, not legal advice.

### Deletion rights

Deletion on this protocol is advisory only. A relay may honor a deletion
request, but nothing requires it to, and any relay, contact, or archive
that already copied a record may keep it indefinitely — publishing here
is not equivalent to deleting or fully controlling that data afterward.
If your jurisdiction grants you a legal right to deletion of personal
data (for example under the EU GDPR or an applicable US state law),
publishing a record through this protocol may not by itself satisfy that
right. Do not publish information here that you may later be legally
required to be able to delete.

### Not built, and not on the roadmap

This protocol does not include Shadow Quant, any cross-token exchange
mechanism, or any mirrored on-chain/ERC-20 ledger of Quants — each would
reintroduce the money-transmission, Sybil, and tax-classification
problems the design exists to avoid.

---

Both disclosures above are also kept as constants
(`qw_protocol::legal::CO_AUTHORSHIP_BOUNDARY_NOTICE`,
`qw_protocol::legal::DELETION_RIGHTS_DISCLOSURE`) so a future client can
display them verbatim instead of re-deriving the wording. Keep the two
copies in sync by hand — same convention as the NIP docs in
`protocol/nips/`.
