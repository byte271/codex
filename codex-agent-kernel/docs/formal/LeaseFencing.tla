------------------------------ MODULE LeaseFencing ------------------------------
\* Fencing for kernel operation leases.
\* TLC can check this with: tlc LeaseFencing
\*
\* Safety: a commit with generation g succeeds only if g equals the current
\* unexpired generation. After Expire, generation advances and the old
\* worker cannot commit. Completed is monotonic.

EXTENDS Naturals

VARIABLES generation, expired, committed, lastCommitGen

TypeOK ==
  /\ generation \in Nat \ {0}
  /\ expired \in BOOLEAN
  /\ committed \in BOOLEAN
  /\ lastCommitGen \in Nat \union {0}

Init ==
  /\ generation = 1
  /\ expired = FALSE
  /\ committed = FALSE
  /\ lastCommitGen = 0

Lease(g) ==
  /\ ~committed
  /\ expired
  /\ g = generation + 1
  /\ generation' = g
  /\ expired' = FALSE
  /\ UNCHANGED <<committed, lastCommitGen>>

Expire ==
  /\ ~committed
  /\ ~expired
  /\ expired' = TRUE
  /\ UNCHANGED <<generation, committed, lastCommitGen>>

Commit(g) ==
  /\ ~committed
  /\ ~expired
  /\ g = generation
  /\ committed' = TRUE
  /\ lastCommitGen' = g
  /\ UNCHANGED <<generation, expired>>

StaleCommit(g) ==
  /\ g # generation \/ expired \/ committed
  /\ UNCHANGED <<generation, expired, committed, lastCommitGen>>

Next ==
  \/ Expire
  \/ \E g \in 1..5 : Lease(g) \/ Commit(g) \/ StaleCommit(g)

Spec == Init /\ [][Next]_<<generation, expired, committed, lastCommitGen>>

\* Invariants
CompletedMonotonic == [](committed => []committed)

NoStaleCommit ==
  committed => (lastCommitGen = generation /\ ~expired)

OneGenerationOwnsCommit ==
  committed => lastCommitGen # 0
=============================================================================
