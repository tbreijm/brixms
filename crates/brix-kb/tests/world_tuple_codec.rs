use brix_canon::CanonWriter;
use brix_kb::world::{
    extract_indexed_field, RelationDecl, TupleRecord, WorldError, WorldTuple, TUPLE_MAGIC_V1,
};

fn indexed_decl() -> RelationDecl {
    RelationDecl::new(
        "orders",
        vec!["order_id".into()],
        vec!["customer_id".into(), "total".into()],
        vec!["customer_id".into()],
    )
}

fn raw_record(magic: &[u8], fields: &[(&str, &str)], trailing: &[u8]) -> WorldTuple {
    let mut writer = CanonWriter::new();
    writer.write_bytes(magic);
    writer.write_uint(fields.len() as u64);
    for (name, value) in fields {
        writer.write_str(name);
        writer.write_bytes(value.as_bytes());
    }
    let mut bytes = writer.finish();
    bytes.extend_from_slice(trailing);
    WorldTuple::new(bytes)
}

#[test]
fn structured_tuple_roundtrips_and_projects_named_field() {
    let mut record = TupleRecord::new();
    record.set_str("total", "99");
    record.set_str("customer_id", "cust-7");
    let tuple = record.to_tuple();

    assert_eq!(TupleRecord::from_tuple(&tuple).unwrap(), record);
    assert_eq!(
        extract_indexed_field("orders", &indexed_decl(), "customer_id", &tuple).unwrap(),
        b"cust-7"
    );
}

#[test]
fn missing_named_field_never_uses_delimited_or_positional_fallback() {
    for payload in ["total=99", "total=99:status=open", "99"] {
        let tuple = WorldTuple::new(payload.as_bytes().to_vec());
        assert!(extract_indexed_field("orders", &indexed_decl(), "customer_id", &tuple).is_err());
    }

    let mut record = TupleRecord::new();
    record.set_str("total", "99");
    assert!(matches!(
        extract_indexed_field("orders", &indexed_decl(), "customer_id", &record.to_tuple()),
        Err(WorldError::MissingIndexField { .. })
    ));
}

#[test]
fn decoder_rejects_duplicate_and_out_of_order_fields() {
    let duplicate = raw_record(
        TUPLE_MAGIC_V1,
        &[("customer_id", "first"), ("customer_id", "last")],
        &[],
    );
    assert!(matches!(
        TupleRecord::from_tuple(&duplicate),
        Err(WorldError::InvalidSchema(message)) if message.contains("duplicate")
    ));

    let out_of_order = raw_record(
        TUPLE_MAGIC_V1,
        &[("total", "99"), ("customer_id", "cust-7")],
        &[],
    );
    assert!(matches!(
        TupleRecord::from_tuple(&out_of_order),
        Err(WorldError::InvalidSchema(message)) if message.contains("canonical order")
    ));
}

#[test]
fn decoder_rejects_trailing_bytes_and_unsupported_versions() {
    let trailing = raw_record(TUPLE_MAGIC_V1, &[("customer_id", "cust-7")], &[0xff]);
    assert!(matches!(
        TupleRecord::from_tuple(&trailing),
        Err(WorldError::InvalidSchema(message)) if message.contains("trailing")
    ));

    let unsupported = raw_record(b"brix.tuple@2\0", &[("customer_id", "cust-7")], &[]);
    assert!(matches!(
        TupleRecord::from_tuple(&unsupported),
        Err(WorldError::InvalidSchema(message)) if message.contains("unsupported tuple record version")
    ));
}
