use batteries_core::db::Db;

fn open_temp() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("batteries.db");
    let db = Db::open(&path).unwrap();
    (dir, db)
}

#[test]
fn create_and_move_records_history() {
    let (_dir, db) = open_temp();
    let battery = db.create(1, "aa", Some(1900), "Test", "Drawer", "").unwrap();
    assert_eq!(battery.r#type, "AA");
    assert_eq!(battery.location, "Drawer");
    assert_eq!(db.history(1).unwrap().len(), 1);

    db.set_location(1, "Charger").unwrap();
    assert_eq!(db.get(1).unwrap().unwrap().location, "Charger");
    assert_eq!(db.history(1).unwrap().len(), 2);
}

#[test]
fn same_location_is_not_recorded_twice() {
    let (_dir, db) = open_temp();
    db.create(1, "AA", None, "", "Drawer", "").unwrap();
    db.set_location(1, "Drawer").unwrap();
    assert_eq!(db.history(1).unwrap().len(), 1);
}

#[test]
fn foreign_keys_are_enabled() {
    let (_dir, db) = open_temp();
    // Deleting an instrument should SET NULL a measurement's instrument_id
    // rather than erroring, which only happens with FKs on and the schema's
    // ON DELETE SET NULL in effect.
    db.create(1, "AA", None, "", "", "").unwrap();
    let instrument = db.add_instrument("Charger", 1).unwrap();
    db.add_measurement(1, "bought", "2026-01-01", None, None, Some(instrument.id), None, None, None, "")
        .unwrap();
    db.delete_instrument(instrument.id).unwrap();
    let measurements = db.measurements(1).unwrap();
    assert_eq!(measurements.len(), 1);
    assert_eq!(measurements[0].instrument_id, None);
}

#[test]
fn deleting_the_last_unused_battery_removes_it_outright() {
    let (_dir, db) = open_temp();
    db.create(1, "AA", None, "", "", "").unwrap();
    let result = db.soft_delete(1).unwrap().unwrap();
    assert_eq!(result.hard_deleted, Some(true));
    assert!(db.get(1).unwrap().is_none());
    assert_eq!(db.next_id().unwrap(), 1);
}

#[test]
fn deleting_a_battery_with_measurements_only_soft_deletes() {
    let (_dir, db) = open_temp();
    db.create(1, "AA", None, "", "", "").unwrap();
    let instrument = db.add_instrument("SkyRC MC3000", 1).unwrap();
    db.add_measurement(1, "bought", "2026-01-01", None, None, Some(instrument.id), None, None, None, "")
        .unwrap();
    let result = db.soft_delete(1).unwrap().unwrap();
    assert_eq!(result.hard_deleted, Some(false));
    let row = db.get(1).unwrap().unwrap();
    assert!(row.deleted_at.is_some());
    assert_eq!(db.next_id().unwrap(), 2);
}

#[test]
fn deleting_a_battery_that_is_not_the_last_id_only_soft_deletes() {
    let (_dir, db) = open_temp();
    db.create(1, "AA", None, "", "", "").unwrap();
    db.create(2, "AA", None, "", "", "").unwrap();
    let result = db.soft_delete(1).unwrap().unwrap();
    assert_eq!(result.hard_deleted, Some(false));
    assert!(db.get(1).unwrap().is_some());
    assert_eq!(db.next_id().unwrap(), 3);
}

#[test]
fn deleting_an_already_deleted_battery_is_a_no_op() {
    let (_dir, db) = open_temp();
    db.create(1, "AA", None, "", "", "").unwrap();
    db.create(2, "AA", None, "", "", "").unwrap(); // keep id 1 from being the max
    let first = db.soft_delete(1).unwrap().unwrap();
    assert_eq!(first.hard_deleted, Some(false));
    let second = db.soft_delete(1).unwrap().unwrap();
    assert_eq!(second.hard_deleted, Some(false));
    assert!(second.deleted_at.is_some());
}

#[test]
fn move_mode_swaps_with_adjacent_sibling_and_survives_a_gap() {
    let (_dir, db) = open_temp();
    let instrument = db.add_instrument("C9000", 1).unwrap();
    let a = db.add_mode(instrument.id, "Refresh", "analysed", "", None).unwrap();
    let b = db.add_mode(instrument.id, "Analyze", "analysed", "", None).unwrap();
    let c = db.add_mode(instrument.id, "Break-in", "analysed", "", None).unwrap();

    // Delete the middle one, leaving a gap in `position`, then move the last
    // one up: it should still land ahead of the first, not error on the gap.
    db.delete_mode(b.id).unwrap();
    assert!(db.move_mode(c.id, "up").unwrap());
    let modes = db.instrument(instrument.id).unwrap().unwrap().modes;
    let ids: Vec<i64> = modes.iter().map(|m| m.id).collect();
    assert_eq!(ids, vec![c.id, a.id]);

    // Already at the top: no-op, returns false.
    assert!(!db.move_mode(c.id, "up").unwrap());
}

#[test]
fn adjacent_ids_counts_deleted_batteries_as_neighbors() {
    let (_dir, db) = open_temp();
    db.create(1, "AA", None, "", "", "").unwrap();
    db.create(2, "AA", None, "", "", "").unwrap();
    db.create(3, "AA", None, "", "", "").unwrap();
    db.soft_delete(2).unwrap();
    let (prev, next) = db.adjacent_ids(3).unwrap();
    assert_eq!(prev, Some(2));
    assert_eq!(next, None);
}

#[test]
fn import_and_reopen_replaces_the_live_database_in_place() {
    let (_dir, mut db) = open_temp();
    db.create(1, "AA", None, "", "", "").unwrap();

    // A second, independent database standing in for an existing
    // batteries.db being imported from a backup.
    let other_dir = tempfile::tempdir().unwrap();
    let other_path = other_dir.path().join("other.db");
    {
        let other = Db::open(&other_path).unwrap();
        other.create(42, "18650", Some(3500), "Imported Brand", "Test Shelf", "").unwrap();
    }

    db.import_and_reopen(&other_path).unwrap();

    // The old data is gone, replaced by the imported database's contents,
    // and the connection is immediately usable without reopening `db`.
    assert!(db.get(1).unwrap().is_none());
    let imported = db.get(42).unwrap().expect("imported battery present");
    assert_eq!(imported.r#type, "18650");
    assert_eq!(imported.brand, "Imported Brand");
    assert_eq!(imported.location, "Test Shelf");

    // The connection still lives at the original path, not the source's.
    assert_eq!(db.create(2, "AAA", None, "", "", "").unwrap().id, 2);
}

#[test]
fn last_reading_defaults_skips_bought_entries() {
    let (_dir, db) = open_temp();
    db.create(1, "AA", None, "", "", "").unwrap();
    let instrument = db.add_instrument("C9000", 1).unwrap();
    db.add_measurement(1, "analysed", "2026-01-01", Some(1900), None, Some(instrument.id), None, Some(500), None, "")
        .unwrap();
    db.add_measurement(1, "bought", "2026-02-01", None, None, None, None, None, None, "").unwrap();
    let (instrument_id, _mode_id, discharge_ma, _charge_ma) = db.last_reading_defaults().unwrap();
    assert_eq!(instrument_id, Some(instrument.id));
    assert_eq!(discharge_ma, Some(500));
}

#[test]
fn atomically_rolls_back_every_write_when_one_step_fails() {
    let (_dir, db) = open_temp();
    db.create(1, "AA", None, "", "Drawer", "").unwrap();

    let result: batteries_core::db::DbResult<()> = db.atomically(|| {
        db.set_location(1, "Torch")?;
        Err(rusqlite::Error::InvalidParameterName("simulated failure".into()))
    });

    assert!(result.is_err());
    assert_eq!(db.get(1).unwrap().unwrap().location, "Drawer", "the move should have been undone");
    assert_eq!(db.history(1).unwrap().len(), 1, "and so should its history entry");
}

#[test]
fn atomic_steps_nest_inside_each_other() {
    let (_dir, db) = open_temp();
    db.create(1, "AA", None, "", "Drawer", "").unwrap();
    db.create(2, "AA", None, "", "Drawer", "").unwrap();

    // set_location is itself atomic; nesting it must not break the outer step.
    let result: batteries_core::db::DbResult<()> = db.atomically(|| {
        db.set_location(1, "Torch")?;
        db.set_location(2, "Torch")?;
        Ok(())
    });

    assert!(result.is_ok());
    assert_eq!(db.get(2).unwrap().unwrap().location, "Torch");
}

#[test]
fn a_new_bought_entry_replaces_the_old_one_whichever_way_it_is_added() {
    let (_dir, db) = open_temp();
    db.create(1, "AA", None, "", "", "").unwrap();

    db.add_measurement(1, "bought", "2026-01-01", None, None, None, None, None, None, "").unwrap();
    db.add_measurement(1, "bought", "2026-02-01", None, None, None, None, None, None, "").unwrap();

    let bought: Vec<_> = db.measurements(1).unwrap().into_iter().filter(|m| m.kind == "bought").collect();
    assert_eq!(bought.len(), 1);
    assert_eq!(bought[0].measured_at, "2026-02-01");
}

#[test]
fn clearing_a_record_drops_its_match_memberships() {
    let (_dir, db) = open_temp();
    for id in 1..=2 {
        db.create(id, "AA", Some(2000), "", "storage", "").unwrap();
    }
    let matched = db.create_match("Torch", "AA", "low", &[1, 2]).unwrap();

    db.reset(1).unwrap();

    assert_eq!(db.match_battery_ids(matched.id).unwrap(), vec![2]);
    // The sticker is on a different cell now: filing that cell in storage
    // must not close the old match or pull battery 2 back.
    let (_, also_returned) = db.set_location_checked(1, "storage", "storage").unwrap();
    assert!(also_returned.is_empty());
    assert_eq!(db.get(2).unwrap().unwrap().location, "Torch");
    assert!(db.list_matches().unwrap()[0].returned_at.is_none());
}
