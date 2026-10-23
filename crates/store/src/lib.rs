//! SQLite persistence of the domain model: schema, migrations, transactions.
//!
//! L1: may depend on `gymgar-domain` only. Every schema change ships with a migration and a test
//! that opens a database written by the previous version. One mutating command = one transaction.
