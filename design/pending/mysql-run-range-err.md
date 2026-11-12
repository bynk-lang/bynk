---
level: patch
changelog: `MySql.run` answers `MySqlFailed` for an `insertId` or row count past 2^53, where it read as `0`
breaking: A `MySql.run` whose `insertId` or `affectedRows` is past 2^53 now answers `Err(MySqlFailed(0, …))`, where it read as `0`; match `MySqlFailed` as for any other failure.
---
