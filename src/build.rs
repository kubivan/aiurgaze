fn main() {
    // Generate Rust types and Tonic services from SC2 protobufs.
    // Assumes you vendor s2clientprotocol under `protos/s2clientprotocol`.
    // You can add as many .proto files as needed; the important one is sc2api.proto.


    let proto_root = "protos/s2clientprotocol";
    let files = vec![
        format!("{}/sc2api.proto", proto_root),
        format!("{}/raw.proto", proto_root),
        format!("{}/spatial.proto", proto_root),
        format!("{}/common.proto", proto_root),
        format!("{}/debug.proto", proto_root),
        format!("{}/query.proto", proto_root),
        format!("{}/score.proto", proto_root),
    ];


    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .out_dir("src/generated")
        .compile(&files, &[proto_root])
        .expect("failed to compile protos");
}