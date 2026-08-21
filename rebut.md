# Final answer:
We thank the reviewers for their valuable feedback!

***Novelty (Rev. A, Rev. C)***
We agree that DACTIE builds on existing primitives - deliberately so, since novel cryptography would weaken the security claims. Our goals G1-G9, challenges C1-C11 and requirements R1-R4 can conflict when components are naively composed. E.g. GossipSub-vanilla's topic subscription leaks receiver interest (C2 ∈ R2 ∩ R1), which our partitioned channels mitigate. However, partitioned channels amplify sender anonymity (R2) leakage if floodpub=true. Deactivating floodpub lowers R2 risk, but increases the potential for eclipse attacks, conflicting with G1/R1. Our contribution is at the system level. The novelty is the concept to put together existing primitives, and modify them thoughtfully to simultaneously address all requirements, not addressed by previous work (Sec. 1,8).

***Authority (Rev. A, Rev. C)*** 
It is true that the AA is centralized and trusted in the current PoC. The Cam20-scheme allows the inclusion of multiple Authorization Authorities, which is left for future work. To demonstrate the feasibility of this PoC, one Authority is sufficient. We will describe DACTIE more precisely as a permissioned, decentralized CTI exchange network with centralised admission. Additionally, we will add that an AA can enroll itself as a DACTIE member.

***Consistant state (Rev. C)***
CTI messages are independent, immutable records without the need for ordering or resolving conflicting state transitions. Archive replicas may consequently contain temporarily different subsets of messages because of propagation delays or selective dissemination; this is an explicit consistency trade-off. Consensus would add ordering, coordination, and storage overhead without strengthening the required message-level authenticity or confidentiality guarantees.

***Threat Model (Rev. A, Rev. C)***
Our threat model allows any subset of peers to be dishonest.
- An existing approach of Dunnet et al. (https://www.sciencedirect.com/science/article/pii/S1389128624005188) includes four different threat actors in their threat model: 1. Unauthorised Sensitive Data Disclosure, 2. the Malicious Producer, 3. the Malicious Delegate, and 4. the Malicious Group Manager, but omits nation-state actors. Some approaches do not even mention a concrete threat model at all (https://link.springer.com/chapter/10.1007/978-3-030-90019-9_9, http://arxiv.org/abs/2103.13158

***Sender Anonymity (Rev. A, Rev. C)***
Sender Anonymity will be around 6.
- Disabling flood_publish reduces the attack surface; it doesn't eliminate first-spy attacks. The author still emits to its D mesh peers simultaneously and first, so an adversary holding one of those ~6 mesh slots still observes the message first-hop. What changes is the cost: the adversary must win a mesh slot (subject to peer scoring and mesh churn) rather than merely appear in a DHT routing table with hundreds of entries. That's a large quantitative improvement, not an unlinkability proof — which is presumably what your P<sub>SML</sub> bound captures. -> Claim reduction and cite the bound. 
- Stronger sender anonymity requires decoupling the publisher from the origin of the broadcast — e.g. a Dandelion++-style stem phase. We deliberately did not adopt this: the added relay latency conflicts with R1, and the single-path stem would weaken censorship resistance in a threat model that already includes message-withholding peers. We note it as future work and state the residual SML exposure explicitly.

***Metadata Anonymity (Rev. A)***
Dandelion (https://arxiv.org/abs/1805.11060) could be a right direction for the remaining leakage. Stronger sender anonymity requires decoupling the publisher from the origin of the broadcast — e.g. a Dandelion++-style stem phase. We deliberately did not adopt this: the added relay latency conflicts with R1, and the single-path stem would weaken censorship resistance in a threat model that already includes message-withholding peers. We note it as future work and state the residual SML exposure explicitly.

***Evaluation Performance (Rev. A)***

***Integrity (Rev. B)***
Strong integrity is guaranteed. It is not possible to alter the signed channel+payload of a message. An altered PeerID will have no valid key package to sent a message (See Section 5.1 and Figure 7) 
***Attribution (Rev. B)*** 
Message attribution is an explicitly mentioned use case. It is the sender's decision if they intent to attribute a message to their MemberID(and built/use their reputation), or use an anonymous PeerID.
    - Can a PeerID be altered by will? I suppose it will loose it's signature

***Motivation (Rev. B)***
    - Look for an explicit example where one participates as CTI-sharer, but needs strong anonymity# Final answer:
We thank the reviewers for their valuable feedback!

***Novelty (Rev. A, Rev. C)***
We agree that DACTIE builds on existing primitives - deliberately so, since novel cryptography would weaken the security claims. Our goals G1-G9, challenges C1-C11 and requirements R1-R4 can conflict when components are naively composed. E.g. GossipSub-vanilla's topic subscription leaks receiver interest (C2 ∈ R2 ∩ R1), which our partitioned channels mitigate. However, partitioned channels amplify sender anonymity (R2) leakage if floodpub=true. Deactivating floodpub lowers R2 risk, but increases the potential for eclipse attacks, conflicting with G1/R1. Our contribution is at the system level. The novelty is the concept to put together existing primitives, and modify them thoughtfully to simultaneously address all requirements, not addressed by previous work (Sec. 1,8).

***Authority (Rev. A, Rev. C)*** 
It is true that the AA is centralized and trusted in the current PoC. The Cam20-scheme allows the inclusion of multiple Authorization Authorities, which is left for future work. To demonstrate the feasibility of this PoC, one Authority is sufficient. We will describe DACTIE more precisely as a permissioned, decentralized CTI exchange network with centralised admission. Additionally, we will add that an AA can enroll itself as a DACTIE member.

***Consistant state (Rev. C)***
CTI messages are independent, immutable records without the need for ordering or resolving conflicting state transitions. Archive replicas may consequently contain temporarily different subsets of messages because of propagation delays or selective dissemination; this is an explicit consistency trade-off. Consensus would add ordering, coordination, and storage overhead without strengthening the required message-level authenticity or confidentiality guarantees.

***Threat Model (Rev. A, Rev. C)***
Our threat model allows any subset of peers to be dishonest.
- An existing approach of Dunnet et al. (https://www.sciencedirect.com/science/article/pii/S1389128624005188) includes four different threat actors in their threat model: 1. Unauthorised Sensitive Data Disclosure, 2. the Malicious Producer, 3. the Malicious Delegate, and 4. the Malicious Group Manager, but omits nation-state actors. Some approaches do not even mention a concrete threat model at all (https://link.springer.com/chapter/10.1007/978-3-030-90019-9_9, http://arxiv.org/abs/2103.13158

***Sender Anonymity (Rev. A, Rev. C)***
Sender Anonymity will be around 6.
- Disabling flood_publish reduces the attack surface; it doesn't eliminate first-spy attacks. The author still emits to its D mesh peers simultaneously and first, so an adversary holding one of those ~6 mesh slots still observes the message first-hop. What changes is the cost: the adversary must win a mesh slot (subject to peer scoring and mesh churn) rather than merely appear in a DHT routing table with hundreds of entries. That's a large quantitative improvement, not an unlinkability proof — which is presumably what your P<sub>SML</sub> bound captures. -> Claim reduction and cite the bound. 
- Stronger sender anonymity requires decoupling the publisher from the origin of the broadcast — e.g. a Dandelion++-style stem phase. We deliberately did not adopt this: the added relay latency conflicts with R1, and the single-path stem would weaken censorship resistance in a threat model that already includes message-withholding peers. We note it as future work and state the residual SML exposure explicitly.

***Metadata Anonymity (Rev. A)***
Dandelion (https://arxiv.org/abs/1805.11060) could be a right direction for the remaining leakage. Stronger sender anonymity requires decoupling the publisher from the origin of the broadcast — e.g. a Dandelion++-style stem phase. We deliberately did not adopt this: the added relay latency conflicts with R1, and the single-path stem would weaken censorship resistance in a threat model that already includes message-withholding peers. We note it as future work and state the residual SML exposure explicitly.

***Evaluation Performance (Rev. A)***

***Integrity (Rev. B)***
Strong integrity is guaranteed. It is not possible to alter the signed channel+payload of a message. An altered PeerID will have no valid key package to sent a message (See Section 5.1 and Figure 7) 
***Attribution (Rev. B)*** 
Message attribution is an explicitly mentioned use case. It is the sender's decision if they intent to attribute a message to their MemberID(and built/use their reputation), or use an anonymous PeerID.
    - Can a PeerID be altered by will? I suppose it will loose it's signature

***Motivation (Rev. B)***
    - Look for an explicit example where one participates as CTI-sharer, but needs strong anonymity.
