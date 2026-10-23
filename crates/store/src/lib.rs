//! Local persistence of the domain model: schema, migrations, atomic writes.
//!
//! L1: may depend on `gymgar-domain` only. Every schema change ships with a migration and a test
//! that opens data written by the previous version.
