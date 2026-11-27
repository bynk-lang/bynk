---
level: patch
changelog: `bynk.deploy.lock` and `BYNK_DEPLOY_FACTS` gain the shapes a supported agent move will record (an `agent_moves` record per move under way, `transferred_classes`, and a facts `transfer` field), with the ledger written as version 2 only while a move is open; nothing writes them yet, so a deploy, its ledger and its facts are unchanged (#3565)
---
