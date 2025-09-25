fn main() -> Result<(), Box<dyn std::error::Error>> {
    // tonic_prost_build::configure()
    //     .build_server(true)
    //     .compile_protos(
    //         &["protos/s2clientprotocol/common.proto",
    //         "protos/s2clientprotocol/debug.proto",
    //         "protos/s2clientprotocol/error.proto",
    //         "protos/s2clientprotocol/query.proto",
    //         "protos/s2clientprotocol/raw.proto",
    //         "protos/s2clientprotocol/sc2api.proto",
    //         "protos/s2clientprotocol/score.proto",
    //         "protos/s2clientprotocol/spatial.proto",
    //         "protos/sc2proxy.proto",
    //         "protos/s2clientprotocol/ui.proto"],
    //             &["protos/"],
    //     )?;
    Ok(())
}
