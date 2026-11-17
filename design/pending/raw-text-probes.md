---
level: patch
changelog: Two gated growth probes, `ts_raw_sites` and `ts_text_idents`, count the raw-text TypeScript escapes (`TsStmt::raw(` and `TsExpr::Ident(format!(`) that `ts_writes` and `verbatim_sites` miss, so neither can grow without a reasoned `raises:`
probes: ts_raw_sites, ts_text_idents
---
