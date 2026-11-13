---
level: minor
changelog: `Vectorize` gains `upsertValue[M: Codable](vectors: List[VectorOf[M]])`, which writes vectors whose metadata is a value of your own type, encoded by its codec as `upsert` with `Json.encode` would (reads stay as text so a stale vector can be skipped), and the Book gains the guide "Search by meaning with Vectorize and Workers AI"
---
