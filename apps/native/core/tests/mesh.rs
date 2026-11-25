//! Checks the mesh readers (`mesh`) on the sample 3D files the sample-files script writes, and
//! that what they write is a glTF the still renderer reads.

use northroom_core::mesh;
use std::path::PathBuf;

fn read(name: &str) -> mesh::Mesh {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/meshes")
        .join(name);
    mesh::read(&path).expect("a mesh")
}

#[test]
fn the_sample_meshes_read_with_their_triangles_and_colours() {
    let bag = read("Coffee bag.ply");
    assert_eq!((bag.positions.len(), bag.triangles.len()), (2854, 5580));
    assert!(bag.colours.is_some());
    let bean = read("Coffee bean.stl");
    assert_eq!(bean.positions.len(), bean.triangles.len() * 3);
    let mug = read("Mug and saucer.obj");
    assert!(mug.triangles.len() >= 4096);
    assert_eq!(mug.colours, None);
    let usd = read("Coffee bag.usda");
    // Three meshes, each in its material's colour: the bag green, the label cream.
    let colours = usd.colours.as_ref().expect("colours");
    assert!(colours.contains(&[0.0137, 0.159, 0.0685]));
    assert!(colours.contains(&[0.9216, 0.8963, 0.8388]));
    for mesh in [bag, bean, mug, usd] {
        let glb = mesh::write_glb(&mesh);
        assert!(glb.starts_with(b"glTF"));
        assert_eq!(
            u32::from_le_bytes(glb[8..12].try_into().unwrap()) as usize,
            glb.len()
        );
    }
}
