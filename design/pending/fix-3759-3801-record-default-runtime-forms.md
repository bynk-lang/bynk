---
level: patch
changelog: A record literal that leaves out a field now fills in its default correctly when the field is a refined type (the value is a value of that type) or a `Decimal` (the value is a decimal number, not its text), so such programs type-check and `Decimal` arithmetic on the field no longer crashes (#3759, #3801)
---
