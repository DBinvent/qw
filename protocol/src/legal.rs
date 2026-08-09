//! Legal/compliance disclosures (§9, `qw-design-faq.md` §8 "Legal &
//! Compliance"). These are **not** legal advice and this module cannot
//! provide any — they are fixed, factual disclosure copy about what the
//! protocol actually does, kept here as the one source of truth so a
//! future UI doesn't have to invent or rephrase it. `todo-impl.md` §9
//! tracks the surrounding compliance work item by item; this file is
//! only the pieces of it that are text, not process (a written tax
//! attorney opinion, in particular, is a human action this repository
//! cannot complete on its own — see §9).
//!
//! Both disclosures below are tested only for containing their required
//! caveats (`tests::*`), not for legal correctness — that guards against
//! someone silently dropping a caveat while editing the wording, not
//! against the wording being wrong in the first place.

/// Show this **before** a user's first signed record, and again anywhere
/// deletion/removal is offered as a UI action — `todo-impl.md` §9:
/// "document this limitation for users explicitly at record-creation
/// time." Grounded in `qw-design-faq.md` §8 "What about deletion
/// rights?": "Records are work history about identifiable people; EU and
/// several US state laws grant deletion rights. Immutable public chains
/// cannot comply. Nostr (advisory) ... can[not fully]."
pub const DELETION_RIGHTS_DISCLOSURE: &str = "\
Deletion on this protocol is advisory only. A relay may honor a deletion \
request, but nothing requires it to, and any relay, contact, or archive \
that already copied a record may keep it indefinitely — publishing here \
is not equivalent to deleting or fully controlling that data afterward. \
If your jurisdiction grants you a legal right to deletion of personal \
data (for example under the EU GDPR or an applicable US state law), \
publishing a record through this protocol may not by itself satisfy \
that right. Do not publish information here that you may later be \
legally required to be able to delete.";

/// Show this before any external pitch, investor conversation, or
/// public description references the co-authorship framing —
/// `todo-impl.md` §9: "state the co-authorship boundary explicitly...
/// before any external pitch materials reference the abstract." Mirrors
/// `abstract.md`'s own "Legal framing: contribution, not barter"
/// section, which already states this boundary; this is the same
/// statement, kept here so the protocol layer and any UI built on it say
/// the same thing without re-deriving it.
pub const CO_AUTHORSHIP_BOUNDARY_NOTICE: &str = "\
This protocol's co-authorship framing is not a general exemption from \
barter/service taxation. It holds only when work is on a declared \
open-source project (the output is public and non-appropriable) and no \
project involved is controlled by the counterparty in a way that \
privatizes the benefit. Direct bilateral work-for-work, or contribution \
to a counterparty-controlled private project, falls outside this \
framing and is the participants' own tax responsibility to assess — \
this protocol does not determine or launder that responsibility. This \
framing has not been confirmed by a written tax attorney opinion; treat \
it as an engineering description of the system's structure, not legal \
advice.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deletion_disclosure_states_advisory_only_and_names_the_risk() {
        let text = DELETION_RIGHTS_DISCLOSURE.to_lowercase();
        assert!(
            text.contains("advisory"),
            "must not drop the core 'advisory only' caveat"
        );
        assert!(
            text.contains("gdpr") || text.contains("legal right to deletion"),
            "must name the actual legal risk, not just describe the mechanism"
        );
    }

    #[test]
    fn co_authorship_notice_keeps_its_two_conditions_and_the_attorney_caveat() {
        let text = CO_AUTHORSHIP_BOUNDARY_NOTICE.to_lowercase();
        assert!(
            text.contains("declared open-source project"),
            "condition 1 must survive edits"
        );
        assert!(
            text.contains("controlled by the counterparty"),
            "condition 2 must survive edits"
        );
        assert!(
            text.contains("tax attorney"),
            "must not drop the 'not yet confirmed by a tax attorney' caveat"
        );
        assert!(
            text.contains("not legal advice") || text.contains("not... legal advice"),
            "must not present itself as legal advice"
        );
    }
}
