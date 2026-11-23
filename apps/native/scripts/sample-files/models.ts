/**
 * The 3D models, built as triangle meshes and written in each format by hand: a glazed mug on its
 * saucer (glTF binary and OBJ), a bag of coffee (USD and PLY with colours) and one roasted bean
 * (binary STL). Lengths are centimetres unless a format says otherwise.
 */
import { COLOUR } from "./story";

export interface Mesh {
  /** x, y, z of each vertex, y up. */
  positions: number[];
  /** Three vertex indices per triangle, counter-clockwise seen from outside. */
  indices: number[];
}

/** One part of a model and the colour it is, as `#rrggbb`, with how rough its surface is (0 to 1). */
export interface Part {
  name: string;
  mesh: Mesh;
  colour: string;
  roughness: number;
}

const subtract = (a: number[], b: number[]) => [a[0]! - b[0]!, a[1]! - b[1]!, a[2]! - b[2]!];
const cross = (a: number[], b: number[]) => [
  a[1]! * b[2]! - a[2]! * b[1]!,
  a[2]! * b[0]! - a[0]! * b[2]!,
  a[0]! * b[1]! - a[1]! * b[0]!,
];
const normalise = (v: number[]) => {
  const length = Math.hypot(v[0]!, v[1]!, v[2]!);
  return length === 0 ? [0, 1, 0] : v.map((value) => value / length);
};
const vertex = (mesh: Mesh, index: number) => mesh.positions.slice(index * 3, index * 3 + 3);

/** The normal of each vertex: the area-weighted average of the triangles that share it. */
export function vertexNormals(mesh: Mesh): number[] {
  const sums = Array.from({ length: mesh.positions.length }, () => 0);
  for (let at = 0; at < mesh.indices.length; at += 3) {
    const [a, b, c] = [mesh.indices[at]!, mesh.indices[at + 1]!, mesh.indices[at + 2]!];
    const face = cross(
      subtract(vertex(mesh, b), vertex(mesh, a)),
      subtract(vertex(mesh, c), vertex(mesh, a)),
    );
    for (const corner of [a, b, c]) {
      for (let axis = 0; axis < 3; axis += 1) sums[corner * 3 + axis]! += face[axis]!;
    }
  }
  const normals: number[] = [];
  for (let index = 0; index < sums.length; index += 3) {
    normals.push(...normalise(sums.slice(index, index + 3)));
  }
  return normals;
}

/** The normal of one triangle. */
export const faceNormal = (mesh: Mesh, at: number) =>
  normalise(
    cross(
      subtract(vertex(mesh, mesh.indices[at + 1]!), vertex(mesh, mesh.indices[at]!)),
      subtract(vertex(mesh, mesh.indices[at + 2]!), vertex(mesh, mesh.indices[at]!)),
    ),
  );

/** Quads between neighbouring rings of `ringSize` vertices, `rings` of them, each ring closed. */
function stitchRings(rings: number, ringSize: number, firstVertex = 0): number[] {
  const indices: number[] = [];
  for (let ring = 0; ring < rings - 1; ring += 1) {
    for (let step = 0; step < ringSize; step += 1) {
      const a = firstVertex + ring * ringSize + step;
      const b = firstVertex + ring * ringSize + ((step + 1) % ringSize);
      const c = a + ringSize;
      const d = b + ringSize;
      indices.push(a, b, c, b, d, c);
    }
  }
  return indices;
}

/**
 * A surface turned on a lathe: `profile` is (radius, height) pairs from the axis outward and
 * round, so the outside faces out. A point given twice makes a sharp edge there.
 */
export function lathe(profile: readonly (readonly [number, number])[], segments: number): Mesh {
  const positions: number[] = [];
  for (const [radius, height] of profile) {
    for (let step = 0; step < segments; step += 1) {
      const angle = (step / segments) * Math.PI * 2;
      positions.push(radius * Math.cos(angle), height, -radius * Math.sin(angle));
    }
  }
  return { positions, indices: stitchRings(profile.length, segments) };
}

/** Moves every vertex of `mesh` by `offset`. */
export const moved = (mesh: Mesh, offset: readonly [number, number, number]): Mesh => ({
  positions: mesh.positions.map((value, index) => value + offset[index % 3]!),
  indices: mesh.indices,
});

/** `mesh` with every triangle wound the other way, so it faces the other side. */
const flipped = (mesh: Mesh): Mesh => ({
  positions: mesh.positions,
  indices: mesh.indices.map((_, at) => mesh.indices[at - (at % 3) + (2 - (at % 3))]!),
});

/** Joins meshes into one. */
export function joined(meshes: readonly Mesh[]): Mesh {
  const positions: number[] = [];
  const indices: number[] = [];
  for (const mesh of meshes) {
    const first = positions.length / 3;
    positions.push(...mesh.positions);
    indices.push(...mesh.indices.map((index) => index + first));
  }
  return { positions, indices };
}

const SAUCER_TOP_CM = 0.55;

/** A curved handle: an oval tube along an arc beside the mug, closed at both ends. */
function handle(): Mesh {
  const centre = [4.25, SAUCER_TOP_CM + 4.3];
  const arcRadius = 1.75;
  const tube = { across: 0.32, deep: 0.5 };
  const steps = 28;
  const around = 16;
  const positions: number[] = [];
  for (let step = 0; step <= steps; step += 1) {
    const angle = (-115 + (230 * step) / steps) * (Math.PI / 180);
    const outward = [Math.cos(angle), Math.sin(angle)];
    for (let turn = 0; turn < around; turn += 1) {
      const theta = (turn / around) * Math.PI * 2;
      const radial = Math.cos(theta) * tube.across;
      const x = centre[0]! + outward[0]! * (arcRadius + radial);
      const y = centre[1]! + outward[1]! * (arcRadius + radial);
      positions.push(x, y, Math.sin(theta) * tube.deep);
    }
  }
  const indices = stitchRings(steps + 1, around);
  // Close each end with a fan around its middle.
  for (const [ring, flip] of [
    [0, true],
    [steps, false],
  ] as const) {
    const first = ring * around;
    const middle = positions.length / 3;
    const ringPoints = Array.from({ length: around }, (_, turn) =>
      positions.slice((first + turn) * 3, (first + turn) * 3 + 3),
    );
    positions.push(
      ...[0, 1, 2].map((axis) => ringPoints.reduce((sum, point) => sum + point[axis]!, 0) / around),
    );
    for (let turn = 0; turn < around; turn += 1) {
      const a = first + turn;
      const b = first + ((turn + 1) % around);
      indices.push(...(flip ? [middle, b, a] : [middle, a, b]));
    }
  }
  // The arc runs counter-clockwise, which turns the tube's rings inside out.
  return flipped({ positions, indices });
}

/** The mug and saucer, as parts: glazed mug, its handle, the coffee in it and the saucer. */
export function mugParts(): Part[] {
  const segments = 72;
  const mugProfile: [number, number][] = [
    [0, 0],
    [3.0, 0],
    [3.0, 0],
    [3.2, 0.25],
    [3.95, 7.4],
    [4.05, 7.55],
    [3.95, 7.65],
    [3.82, 7.55],
    [3.82, 7.55],
    [3.1, 0.9],
    [0, 0.9],
  ];
  const coffeeHeight = 6.6;
  const coffeeRadius = 3.1 + (3.82 - 3.1) * ((coffeeHeight - 0.9) / (7.55 - 0.9)) + 0.02;
  const saucerProfile: [number, number][] = [
    [0, 0],
    [4.2, 0],
    [4.2, 0],
    [4.5, 0.12],
    [5.4, 0.3],
    [7.4, 0.95],
    [7.7, 1.12],
    [7.62, 1.3],
    [7.4, 1.25],
    [5.0, 0.72],
    [3.9, SAUCER_TOP_CM],
    [0, SAUCER_TOP_CM],
  ];
  const lift = [0, SAUCER_TOP_CM, 0] as const;
  return [
    {
      name: "Mug",
      mesh: moved(lathe(mugProfile, segments), lift),
      colour: COLOUR.green,
      roughness: 0.22,
    },
    { name: "Handle", mesh: handle(), colour: COLOUR.green, roughness: 0.22 },
    {
      name: "Coffee",
      mesh: moved(
        lathe(
          [
            [coffeeRadius, coffeeHeight],
            [0, coffeeHeight],
          ],
          segments,
        ),
        lift,
      ),
      colour: "#3a2314",
      roughness: 0.08,
    },
    { name: "Saucer", mesh: lathe(saucerProfile, segments), colour: COLOUR.paper, roughness: 0.4 },
  ];
}

const BAG = { heightCm: 20, halfWidthCm: 6.5, halfDepthCm: 3.8, sealCm: 1.6 } as const;

/** Where a bag's surface is at height `v` (0 to 1) and angle `t`: a pouch that flattens to its seal. */
function bagPoint(v: number, t: number): [number, number, number] {
  const halfDepth = BAG.halfDepthCm * (1 - v) ** 0.55 + 0.12;
  const halfWidth = BAG.halfWidthCm + 0.35 * Math.sin(Math.PI * v);
  const exponent = 0.5;
  const c = Math.cos(t);
  const s = Math.sin(t);
  const x = halfWidth * Math.sign(c) * Math.abs(c) ** exponent;
  const z = halfDepth * Math.sign(s) * Math.abs(s) ** exponent;
  return [x, v * BAG.heightCm, z];
}

/** The bag, as parts: its body, the printed label on its front and the crimped seal on top. */
export function bagParts(): Part[] {
  const rings = 40;
  const around = 64;
  const body: number[] = [];
  for (let ring = 0; ring <= rings; ring += 1) {
    for (let step = 0; step < around; step += 1) {
      body.push(...bagPoint(ring / rings, -(step / around) * Math.PI * 2));
    }
  }
  const indices = stitchRings(rings + 1, around);
  const bottomMiddle = body.length / 3;
  body.push(0, 0, 0);
  for (let step = 0; step < around; step += 1)
    indices.push(bottomMiddle, (step + 1) % around, step);

  // The label sits just proud of the front, which faces +z.
  const label: number[] = [];
  const labelRows = 12;
  const labelColumns = 16;
  for (let row = 0; row <= labelRows; row += 1) {
    const v = 0.22 + (0.42 * row) / labelRows;
    for (let column = 0; column <= labelColumns; column += 1) {
      const u = -0.62 + (1.24 * column) / labelColumns;
      const halfDepth = BAG.halfDepthCm * (1 - v) ** 0.55 + 0.12;
      const halfWidth = BAG.halfWidthCm + 0.35 * Math.sin(Math.PI * v);
      const x = u * halfWidth;
      // The front follows the pouch's rounded-box section, (x/w)^4 + (z/d)^4 = 1.
      const z = halfDepth * Math.max(1 - Math.abs(u) ** 4, 0) ** 0.25 + 0.03;
      label.push(x, v * BAG.heightCm, z);
    }
  }
  const labelIndices: number[] = [];
  for (let row = 0; row < labelRows; row += 1) {
    for (let column = 0; column < labelColumns; column += 1) {
      const a = row * (labelColumns + 1) + column;
      const b = a + 1;
      const c = a + labelColumns + 1;
      const d = c + 1;
      labelIndices.push(a, b, c, b, d, c);
    }
  }

  const sealWidth = BAG.halfWidthCm + 0.05;
  const bottom = BAG.heightCm - 0.2;
  const top = BAG.heightCm + BAG.sealCm;
  const depth = 0.18;
  const seal: Mesh = {
    positions: [
      -sealWidth,
      bottom,
      -depth,
      sealWidth,
      bottom,
      -depth,
      sealWidth,
      top,
      -depth,
      -sealWidth,
      top,
      -depth,
      -sealWidth,
      bottom,
      depth,
      sealWidth,
      bottom,
      depth,
      sealWidth,
      top,
      depth,
      -sealWidth,
      top,
      depth,
    ],
    indices: [
      0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4, 3, 7, 6, 3, 6, 2, 0, 4, 7, 0, 7, 3, 1,
      2, 6, 1, 6, 5,
    ],
  };
  return [
    { name: "Bag", mesh: { positions: body, indices }, colour: COLOUR.green, roughness: 0.55 },
    {
      name: "Label",
      mesh: { positions: label, indices: labelIndices },
      colour: COLOUR.paper,
      roughness: 0.8,
    },
    { name: "Seal", mesh: seal, colour: COLOUR.greenDark, roughness: 0.5 },
  ];
}

/** One roasted bean in millimetres: an ellipsoid, flat underneath, with its crease along the flat. */
export function beanMesh(): Mesh {
  const lengthMm = 9.5;
  const widthMm = 6.6;
  const heightMm = 4.4;
  const rings = 48;
  const around = 48;
  const positions: number[] = [];
  for (let ring = 0; ring <= rings; ring += 1) {
    const u = (ring / rings) * Math.PI;
    for (let step = 0; step < around; step += 1) {
      const v = (step / around) * Math.PI * 2;
      const x = lengthMm * Math.cos(u) * -1;
      const y = widthMm * Math.sin(u) * Math.cos(v);
      let z = heightMm * Math.sin(u) * Math.sin(v);
      if (z < 0) {
        z *= 0.45;
        const creaseY = 0.9 * Math.sin((Math.PI * x) / lengthMm);
        const groove = 1.4 * Math.exp(-(((y - creaseY) / 0.75) ** 2)) * Math.sin(u) ** 0.6;
        z += groove;
      }
      positions.push(x, z, y);
    }
  }
  // Its rings run along x with y and z swapped, which turns them inside out.
  return flipped({ positions, indices: stitchRings(rings + 1, around) });
}

/** An sRGB `#rrggbb` as linear 0 to 1 channels, which glTF and USD expect. */
export function linearColour(hex: string): [number, number, number] {
  const channel = (offset: number) => {
    const value = parseInt(hex.slice(offset, offset + 2), 16) / 255;
    return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  };
  return [channel(1), channel(3), channel(5)];
}

const GLB_MAGIC = 0x46546c67;
const GLB_JSON = 0x4e4f534a;
const GLB_BIN = 0x004e4942;
const FLOAT = 5126;
const UNSIGNED_INT = 5125;
const ARRAY_BUFFER = 34962;
const ELEMENT_ARRAY_BUFFER = 34963;
const METRES_PER_CM = 0.01;

/** A binary glTF 2.0 of `parts`, in metres, each part a primitive with its own material. */
export function glbFile(parts: readonly Part[], generator: string): Buffer {
  const chunks: Buffer[] = [];
  let byteLength = 0;
  const bufferViews: object[] = [];
  const accessors: object[] = [];
  const addView = (data: Buffer, target: number) => {
    bufferViews.push({ buffer: 0, byteOffset: byteLength, byteLength: data.length, target });
    chunks.push(data);
    byteLength += data.length;
    return bufferViews.length - 1;
  };
  const primitives = parts.map((part, material) => {
    const positions = part.mesh.positions.map((value) => value * METRES_PER_CM);
    const normals = vertexNormals(part.mesh);
    const count = positions.length / 3;
    const min = [0, 1, 2].map((axis) =>
      Math.min(...positions.filter((_, index) => index % 3 === axis)),
    );
    const max = [0, 1, 2].map((axis) =>
      Math.max(...positions.filter((_, index) => index % 3 === axis)),
    );
    const position = addView(Buffer.from(new Float32Array(positions).buffer), ARRAY_BUFFER);
    accessors.push({ bufferView: position, componentType: FLOAT, count, type: "VEC3", min, max });
    const normal = addView(Buffer.from(new Float32Array(normals).buffer), ARRAY_BUFFER);
    accessors.push({ bufferView: normal, componentType: FLOAT, count, type: "VEC3" });
    const indices = addView(
      Buffer.from(new Uint32Array(part.mesh.indices).buffer),
      ELEMENT_ARRAY_BUFFER,
    );
    accessors.push({
      bufferView: indices,
      componentType: UNSIGNED_INT,
      count: part.mesh.indices.length,
      type: "SCALAR",
    });
    const first = accessors.length - 3;
    return { attributes: { POSITION: first, NORMAL: first + 1 }, indices: first + 2, material };
  });
  const gltf = {
    asset: { version: "2.0", generator },
    scene: 0,
    scenes: [{ nodes: [0] }],
    nodes: [{ name: "Mug and saucer", mesh: 0 }],
    meshes: [{ name: "Mug and saucer", primitives }],
    materials: parts.map((part) => ({
      name: part.name,
      pbrMetallicRoughness: {
        baseColorFactor: [...linearColour(part.colour), 1],
        metallicFactor: 0,
        roughnessFactor: part.roughness,
      },
    })),
    buffers: [{ byteLength }],
    bufferViews,
    accessors,
  };
  const pad = (data: Buffer, fill: number) =>
    Buffer.concat([data, Buffer.alloc((4 - (data.length % 4)) % 4, fill)]);
  const json = pad(Buffer.from(JSON.stringify(gltf)), 0x20);
  const bin = pad(Buffer.concat(chunks), 0);
  const header = Buffer.alloc(12);
  header.writeUInt32LE(GLB_MAGIC, 0);
  header.writeUInt32LE(2, 4);
  header.writeUInt32LE(12 + 8 + json.length + 8 + bin.length, 8);
  const chunkHeader = (length: number, type: number) => {
    const chunk = Buffer.alloc(8);
    chunk.writeUInt32LE(length, 0);
    chunk.writeUInt32LE(type, 4);
    return chunk;
  };
  return Buffer.concat([
    header,
    chunkHeader(json.length, GLB_JSON),
    json,
    chunkHeader(bin.length, GLB_BIN),
    bin,
  ]);
}

const fixed = (value: number) => {
  const text = value.toFixed(4);
  return text === "-0.0000" ? "0.0000" : text;
};

/** A Wavefront OBJ of `parts`, each its own group, with vertex normals. */
export function objFile(parts: readonly Part[], title: string): string {
  const lines = [`# ${title}`, "# Units: centimetres, y up", ""];
  let first = 1;
  for (const part of parts) {
    lines.push(`o ${part.name}`);
    const { positions, indices } = part.mesh;
    const normals = vertexNormals(part.mesh);
    for (let at = 0; at < positions.length; at += 3) {
      lines.push(
        `v ${fixed(positions[at]!)} ${fixed(positions[at + 1]!)} ${fixed(positions[at + 2]!)}`,
      );
    }
    for (let at = 0; at < normals.length; at += 3) {
      lines.push(`vn ${fixed(normals[at]!)} ${fixed(normals[at + 1]!)} ${fixed(normals[at + 2]!)}`);
    }
    for (let at = 0; at < indices.length; at += 3) {
      const corner = (index: number) => `${index + first}//${index + first}`;
      lines.push(
        `f ${corner(indices[at]!)} ${corner(indices[at + 1]!)} ${corner(indices[at + 2]!)}`,
      );
    }
    first += positions.length / 3;
    lines.push("");
  }
  return lines.join("\n");
}

const tuple = (values: readonly number[]) => `(${values.map(fixed).join(", ")})`;

/** A USD scene, as text, of `parts` under one root, each with a UsdPreviewSurface material. */
export function usdaFile(parts: readonly Part[], root: string): string {
  const meshes = parts.map((part) => {
    const { positions, indices } = part.mesh;
    const normals = vertexNormals(part.mesh);
    const points: string[] = [];
    const pointNormals: string[] = [];
    for (let at = 0; at < positions.length; at += 3) {
      points.push(tuple(positions.slice(at, at + 3)));
      pointNormals.push(tuple(normals.slice(at, at + 3)));
    }
    const axis = (offset: number) => positions.filter((_, index) => index % 3 === offset);
    const low = [0, 1, 2].map((offset) => Math.min(...axis(offset)));
    const high = [0, 1, 2].map((offset) => Math.max(...axis(offset)));
    return `    def Mesh "${part.name}" (
        prepend apiSchemas = ["MaterialBindingAPI"]
    )
    {
        float3[] extent = [${tuple(low)}, ${tuple(high)}]
        int[] faceVertexCounts = [${Array.from({ length: indices.length / 3 }, () => 3).join(", ")}]
        int[] faceVertexIndices = [${indices.join(", ")}]
        point3f[] points = [${points.join(", ")}]
        normal3f[] normals = [${pointNormals.join(", ")}] (
            interpolation = "vertex"
        )
        uniform token subdivisionScheme = "none"
        rel material:binding = </${root}/Materials/${part.name}>
    }
`;
  });
  const materials = parts.map(
    (part) => `        def Material "${part.name}"
        {
            token outputs:surface.connect = </${root}/Materials/${part.name}/Surface.outputs:surface>

            def Shader "Surface"
            {
                uniform token info:id = "UsdPreviewSurface"
                color3f inputs:diffuseColor = ${tuple(linearColour(part.colour))}
                float inputs:metallic = 0
                float inputs:roughness = ${fixed(part.roughness)}
                token outputs:surface
            }
        }
`,
  );
  return `#usda 1.0
(
    defaultPrim = "${root}"
    metersPerUnit = 0.01
    upAxis = "Y"
)

def Xform "${root}" (
    kind = "component"
)
{
${meshes.join("\n")}
    def Scope "Materials"
    {
${materials.join("\n")}    }
}
`;
}

/** A binary PLY of `parts` joined, each vertex in its part's colour, with normals. */
export function plyFile(parts: readonly Part[], comment: string): Buffer {
  const vertices: Buffer[] = [];
  const faces: Buffer[] = [];
  let first = 0;
  for (const part of parts) {
    const { positions, indices } = part.mesh;
    const normals = vertexNormals(part.mesh);
    const colour = [1, 3, 5].map((offset) => parseInt(part.colour.slice(offset, offset + 2), 16));
    for (let at = 0; at < positions.length; at += 3) {
      const record = Buffer.alloc(27);
      for (let axis = 0; axis < 3; axis += 1) {
        record.writeFloatLE(positions[at + axis]!, axis * 4);
        record.writeFloatLE(normals[at + axis]!, 12 + axis * 4);
        record.writeUInt8(colour[axis]!, 24 + axis);
      }
      vertices.push(record);
    }
    for (let at = 0; at < indices.length; at += 3) {
      const record = Buffer.alloc(13);
      record.writeUInt8(3, 0);
      for (let corner = 0; corner < 3; corner += 1)
        record.writeInt32LE(indices[at + corner]! + first, 1 + corner * 4);
      faces.push(record);
    }
    first += positions.length / 3;
  }
  const header = [
    "ply",
    "format binary_little_endian 1.0",
    `comment ${comment}`,
    `element vertex ${vertices.length}`,
    "property float x",
    "property float y",
    "property float z",
    "property float nx",
    "property float ny",
    "property float nz",
    "property uchar red",
    "property uchar green",
    "property uchar blue",
    `element face ${faces.length}`,
    "property list uchar int vertex_indices",
    "end_header",
    "",
  ].join("\n");
  return Buffer.concat([Buffer.from(header, "ascii"), ...vertices, ...faces]);
}

/** A binary STL of `mesh`, each triangle with its own normal. */
export function stlFile(mesh: Mesh, title: string): Buffer {
  const triangles = mesh.indices.length / 3;
  const header = Buffer.alloc(84);
  header.write(title.slice(0, 79), 0, "ascii");
  header.writeUInt32LE(triangles, 80);
  const body = Buffer.alloc(triangles * 50);
  for (let triangle = 0; triangle < triangles; triangle += 1) {
    const at = triangle * 50;
    const normal = faceNormal(mesh, triangle * 3);
    const corners = [0, 1, 2].map((corner) => vertex(mesh, mesh.indices[triangle * 3 + corner]!));
    [normal, ...corners].forEach((point, slot) => {
      for (let axis = 0; axis < 3; axis += 1)
        body.writeFloatLE(point[axis]!, at + slot * 12 + axis * 4);
    });
  }
  return Buffer.concat([header, body]);
}
