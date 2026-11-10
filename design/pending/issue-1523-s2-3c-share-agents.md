---
level: patch
changelog: The project analysis shares each context's agent declarations with the sequence and boundary info it retains instead of deep-copying them twice per context, which shaves editor edit latency on projects with many agents
---
