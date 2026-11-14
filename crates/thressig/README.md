## Thressig
This package implements the [Short Threshold Dynamic Group Signatures](https://eprint.iacr.org/2020/016.pdf) scheme from Camenisch et al. in a modified version. For our use case a single issuing authority is sufficient. Therefore, some steps of the original protocol can be simplified.

The credential is Cam20's variant of the Pointcheval–Sanders signature (section 4.1): with
`(a', h) = H0(id)`, a scalar and a base **hashed into G1**, the issuer signs the member's `sk` as
`σ1 = h`, `σ2 = h^{x + y1·sk + y2·a'}`, and a group signature proves knowledge of `(sk, a')`
(section 4.2, GSign). The base must not be `g` raised to a known exponent: then two members'
credentials reveal `g^x` and `g^{y1}`, and anyone can make a credential on a key no issuer has
seen, whose signatures no archive can open (`test_two_members_cannot_mint_a_credential`).
Opening checks `e(σ1, X̃ Ỹ2^{a'(id)}) · Π T^w = e(σ2, g̃)` per member; sharing peers, whose
shares carry no `a'`, are matched against the opened member (`open_peers_of`).