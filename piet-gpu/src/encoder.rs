// Copyright 2021 The piet-gpu authors.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
// Also licensed under MIT license, at your choice.

//! Low-level scene encoding.

use crate::Blend;
use bytemuck::{Pod, Zeroable};
use piet_gpu_hal::BufWrite;

use crate::stages::{
    self, Config, PathEncoder, Transform, CLIP_PART_SIZE, DRAW_PART_SIZE, PATHSEG_PART_SIZE,
    TRANSFORM_PART_SIZE,
};

pub struct Encoder {
    transform_stream: Vec<stages::Transform>,
    tag_stream: Vec<u8>,
    pathseg_stream: Vec<u8>,
    linewidth_stream: Vec<f32>,
    drawtag_stream: Vec<u32>,
    drawdata_stream: Vec<u8>,
    n_path: u32,
    n_pathseg: u32,
    n_clip: u32,
}

#[derive(Copy, Clone, Debug)]
pub struct EncodedSceneRef<'a, T: Copy + Pod> {
    pub transform_stream: &'a [T],
    pub tag_stream: &'a [u8],
    pub pathseg_stream: &'a [u8],
    pub linewidth_stream: &'a [f32],
    pub drawtag_stream: &'a [u32],
    pub drawdata_stream: &'a [u8],
    pub n_path: u32,
    pub n_pathseg: u32,
    pub n_clip: u32,
    pub ramp_data: &'a [u32],
}

impl<'a, T: Copy + Pod> EncodedSceneRef<'a, T> {
    /// Return a config for the element processing pipeline.
    ///
    /// This does not include further pipeline processing. Also returns the
    /// beginning of free memory.
    pub fn stage_config(&self) -> (Config, usize) {
        // Layout of scene buffer
        let drawtag_offset = 0;
        let n_drawobj = self.n_drawobj();
        let n_drawobj_padded = align_up(n_drawobj, DRAW_PART_SIZE as usize);
        let drawdata_offset = drawtag_offset + n_drawobj_padded * DRAWTAG_SIZE;
        let trans_offset = drawdata_offset + self.drawdata_stream.len();
        let n_trans = self.transform_stream.len();
        let n_trans_padded = align_up(n_trans, TRANSFORM_PART_SIZE as usize);
        let linewidth_offset = trans_offset + n_trans_padded * TRANSFORM_SIZE;
        let n_linewidth = self.linewidth_stream.len();
        let pathtag_offset = linewidth_offset + n_linewidth * LINEWIDTH_SIZE;
        let n_pathtag = self.tag_stream.len();
        let n_pathtag_padded = align_up(n_pathtag, PATHSEG_PART_SIZE as usize);
        let pathseg_offset = pathtag_offset + n_pathtag_padded;

        // Layout of memory
        let mut alloc = 0;
        let trans_alloc = alloc;
        alloc += trans_alloc + n_trans_padded * TRANSFORM_SIZE;
        let pathseg_alloc = alloc;
        alloc += pathseg_alloc + self.n_pathseg as usize * PATHSEG_SIZE;
        let path_bbox_alloc = alloc;
        let n_path = self.n_path as usize;
        alloc += path_bbox_alloc + n_path * PATH_BBOX_SIZE;
        let drawmonoid_alloc = alloc;
        alloc += n_drawobj_padded * DRAWMONOID_SIZE;
        let anno_alloc = alloc;
        alloc += n_drawobj * ANNOTATED_SIZE;
        let clip_alloc = alloc;
        let n_clip = self.n_clip as usize;
        const CLIP_SIZE: usize = 4;
        alloc += n_clip * CLIP_SIZE;
        let clip_bic_alloc = alloc;
        const CLIP_BIC_SIZE: usize = 8;
        // This can round down, as we only reduce the prefix
        alloc += (n_clip / CLIP_PART_SIZE as usize) * CLIP_BIC_SIZE;
        let clip_stack_alloc = alloc;
        const CLIP_EL_SIZE: usize = 20;
        alloc += n_clip * CLIP_EL_SIZE;
        let clip_bbox_alloc = alloc;
        const CLIP_BBOX_SIZE: usize = 16;
        alloc += align_up(n_clip as usize, CLIP_PART_SIZE as usize) * CLIP_BBOX_SIZE;
        let draw_bbox_alloc = alloc;
        alloc += n_drawobj * DRAW_BBOX_SIZE;
        let drawinfo_alloc = alloc;
        // TODO: not optimized; it can be accumulated during encoding or summed from drawtags
        const MAX_DRAWINFO_SIZE: usize = 44;
        alloc += n_drawobj * MAX_DRAWINFO_SIZE;

        let config = Config {
            n_elements: n_drawobj as u32,
            n_pathseg: self.n_pathseg,
            pathseg_alloc: pathseg_alloc as u32,
            anno_alloc: anno_alloc as u32,
            trans_alloc: trans_alloc as u32,
            path_bbox_alloc: path_bbox_alloc as u32,
            drawmonoid_alloc: drawmonoid_alloc as u32,
            clip_alloc: clip_alloc as u32,
            clip_bic_alloc: clip_bic_alloc as u32,
            clip_stack_alloc: clip_stack_alloc as u32,
            clip_bbox_alloc: clip_bbox_alloc as u32,
            draw_bbox_alloc: draw_bbox_alloc as u32,
            drawinfo_alloc: drawinfo_alloc as u32,
            n_trans: n_trans as u32,
            n_path: self.n_path,
            n_clip: self.n_clip,
            trans_offset: trans_offset as u32,
            linewidth_offset: linewidth_offset as u32,
            pathtag_offset: pathtag_offset as u32,
            pathseg_offset: pathseg_offset as u32,
            drawtag_offset: drawtag_offset as u32,
            drawdata_offset: drawdata_offset as u32,
            ..Default::default()
        };
        (config, alloc)
    }

    pub fn write_scene(&self, buf: &mut BufWrite) {
        buf.extend_slice(&self.drawtag_stream);
        let n_drawobj = self.drawtag_stream.len();
        buf.fill_zero(padding(n_drawobj, DRAW_PART_SIZE as usize) * DRAWTAG_SIZE);
        buf.extend_slice(&self.drawdata_stream);
        buf.extend_slice(&self.transform_stream);
        let n_trans = self.transform_stream.len();
        buf.fill_zero(padding(n_trans, TRANSFORM_PART_SIZE as usize) * TRANSFORM_SIZE);
        buf.extend_slice(&self.linewidth_stream);
        buf.extend_slice(&self.tag_stream);
        let n_pathtag = self.tag_stream.len();
        buf.fill_zero(padding(n_pathtag, PATHSEG_PART_SIZE as usize));
        buf.extend_slice(&self.pathseg_stream);
    }

    /// The number of draw objects in the draw object stream.
    pub(crate) fn n_drawobj(&self) -> usize {
        self.drawtag_stream.len()
    }

    /// The number of paths.
    pub(crate) fn n_path(&self) -> u32 {
        self.n_path
    }

    /// The number of path segments.
    pub(crate) fn n_pathseg(&self) -> u32 {
        self.n_pathseg
    }

    pub(crate) fn n_transform(&self) -> usize {
        self.transform_stream.len()
    }

    /// The number of tags in the path stream.
    pub(crate) fn n_pathtag(&self) -> usize {
        self.tag_stream.len()
    }

    pub(crate) fn n_clip(&self) -> u32 {
        self.n_clip
    }
}

/// A scene fragment encoding a glyph.
///
/// This is a reduced version of the full encoder.
#[derive(Default)]
pub struct GlyphEncoder {
    tag_stream: Vec<u8>,
    pathseg_stream: Vec<u8>,
    drawtag_stream: Vec<u32>,
    drawdata_stream: Vec<u8>,
    n_path: u32,
    n_pathseg: u32,
}

const TRANSFORM_SIZE: usize = 24;
const LINEWIDTH_SIZE: usize = 4;
const PATHSEG_SIZE: usize = 52;
const PATH_BBOX_SIZE: usize = 24;
const DRAWMONOID_SIZE: usize = 16;
const DRAW_BBOX_SIZE: usize = 16;
const DRAWTAG_SIZE: usize = 4;
const ANNOTATED_SIZE: usize = 40;

// Tags for draw objects. See shader/drawtag.h for the authoritative source.
const DRAWTAG_FILLCOLOR: u32 = 0x44;
const DRAWTAG_FILLLINGRADIENT: u32 = 0x114;
const DRAWTAG_FILLRADGRADIENT: u32 = 0x2dc;
const DRAWTAG_BEGINCLIP: u32 = 0x05;
const DRAWTAG_ENDCLIP: u32 = 0x25;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Zeroable, Pod)]
pub struct FillColor {
    rgba_color: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Zeroable, Pod)]
pub struct FillLinGradient {
    index: u32,
    p0: [f32; 2],
    p1: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Zeroable, Pod)]
pub struct FillRadGradient {
    index: u32,
    p0: [f32; 2],
    p1: [f32; 2],
    r0: f32,
    r1: f32,
}

#[allow(unused)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Zeroable, Pod)]
pub struct FillImage {
    index: u32,
    // [i16; 2]
    offset: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Zeroable, Pod)]
pub struct Clip {
    blend: u32,
}

impl Encoder {
    pub fn new() -> Encoder {
        Encoder {
            transform_stream: vec![Transform::IDENTITY],
            tag_stream: Vec::new(),
            pathseg_stream: Vec::new(),
            linewidth_stream: vec![-1.0],
            drawtag_stream: Vec::new(),
            drawdata_stream: Vec::new(),
            n_path: 0,
            n_pathseg: 0,
            n_clip: 0,
        }
    }

    pub fn path_encoder(&mut self) -> PathEncoder {
        PathEncoder::new(&mut self.tag_stream, &mut self.pathseg_stream)
    }

    pub fn finish_path(&mut self, n_pathseg: u32) {
        self.n_path += 1;
        self.n_pathseg += n_pathseg;
    }

    pub fn transform(&mut self, transform: Transform) {
        self.tag_stream.push(0x20);
        self.transform_stream.push(transform);
    }

    // Swap the last two tags in the tag stream; used for transformed
    // gradients.
    pub fn swap_last_tags(&mut self) {
        let len = self.tag_stream.len();
        self.tag_stream.swap(len - 1, len - 2);
    }

    // -1.0 means "fill"
    pub fn linewidth(&mut self, linewidth: f32) {
        self.tag_stream.push(0x40);
        self.linewidth_stream.push(linewidth);
    }

    /// Encode a fill color draw object.
    ///
    /// This should be encoded after a path.
    pub fn fill_color(&mut self, rgba_color: u32) {
        self.drawtag_stream.push(DRAWTAG_FILLCOLOR);
        let element = FillColor { rgba_color };
        self.drawdata_stream.extend(bytemuck::bytes_of(&element));
    }

    /// Encode a fill linear gradient draw object.
    ///
    /// This should be encoded after a path.
    pub fn fill_lin_gradient(&mut self, index: u32, p0: [f32; 2], p1: [f32; 2]) {
        self.drawtag_stream.push(DRAWTAG_FILLLINGRADIENT);
        let element = FillLinGradient { index, p0, p1 };
        self.drawdata_stream.extend(bytemuck::bytes_of(&element));
    }

    /// Encode a fill radial gradient draw object.
    ///
    /// This should be encoded after a path.
    pub fn fill_rad_gradient(&mut self, index: u32, p0: [f32; 2], p1: [f32; 2], r0: f32, r1: f32) {
        self.drawtag_stream.push(DRAWTAG_FILLRADGRADIENT);
        let element = FillRadGradient {
            index,
            p0,
            p1,
            r0,
            r1,
        };
        self.drawdata_stream.extend(bytemuck::bytes_of(&element));
    }

    /// Start a clip.
    pub fn begin_clip(&mut self, blend: Option<Blend>) {
        self.drawtag_stream.push(DRAWTAG_BEGINCLIP);
        let element = Clip {
            blend: blend.unwrap_or(Blend::default()).pack(),
        };
        self.drawdata_stream.extend(bytemuck::bytes_of(&element));
        self.n_clip += 1;
    }

    pub fn end_clip(&mut self, blend: Option<Blend>) {
        self.drawtag_stream.push(DRAWTAG_ENDCLIP);
        let element = Clip {
            blend: blend.unwrap_or(Blend::default()).pack(),
        };
        self.drawdata_stream.extend(bytemuck::bytes_of(&element));
        // This is a dummy path, and will go away with the new clip impl.
        self.tag_stream.push(0x10);
        self.n_path += 1;
        self.n_clip += 1;
    }

    /// Return a config for the element processing pipeline.
    ///
    /// This does not include further pipeline processing. Also returns the
    /// beginning of free memory.
    pub fn stage_config(&self) -> (Config, usize) {
        // Layout of scene buffer
        let drawtag_offset = 0;
        let n_drawobj = self.n_drawobj();
        let n_drawobj_padded = align_up(n_drawobj, DRAW_PART_SIZE as usize);
        let drawdata_offset = drawtag_offset + n_drawobj_padded * DRAWTAG_SIZE;
        let trans_offset = drawdata_offset + self.drawdata_stream.len();
        let n_trans = self.transform_stream.len();
        let n_trans_padded = align_up(n_trans, TRANSFORM_PART_SIZE as usize);
        let linewidth_offset = trans_offset + n_trans_padded * TRANSFORM_SIZE;
        let n_linewidth = self.linewidth_stream.len();
        let pathtag_offset = linewidth_offset + n_linewidth * LINEWIDTH_SIZE;
        let n_pathtag = self.tag_stream.len();
        let n_pathtag_padded = align_up(n_pathtag, PATHSEG_PART_SIZE as usize);
        let pathseg_offset = pathtag_offset + n_pathtag_padded;

        // Layout of memory
        let mut alloc = 0;
        let trans_alloc = alloc;
        alloc += trans_alloc + n_trans_padded * TRANSFORM_SIZE;
        let pathseg_alloc = alloc;
        alloc += pathseg_alloc + self.n_pathseg as usize * PATHSEG_SIZE;
        let path_bbox_alloc = alloc;
        let n_path = self.n_path as usize;
        alloc += path_bbox_alloc + n_path * PATH_BBOX_SIZE;
        let drawmonoid_alloc = alloc;
        alloc += n_drawobj_padded * DRAWMONOID_SIZE;
        let anno_alloc = alloc;
        alloc += n_drawobj * ANNOTATED_SIZE;
        let clip_alloc = alloc;
        let n_clip = self.n_clip as usize;
        const CLIP_SIZE: usize = 4;
        alloc += n_clip * CLIP_SIZE;
        let clip_bic_alloc = alloc;
        const CLIP_BIC_SIZE: usize = 8;
        // This can round down, as we only reduce the prefix
        alloc += (n_clip / CLIP_PART_SIZE as usize) * CLIP_BIC_SIZE;
        let clip_stack_alloc = alloc;
        const CLIP_EL_SIZE: usize = 20;
        alloc += n_clip * CLIP_EL_SIZE;
        let clip_bbox_alloc = alloc;
        const CLIP_BBOX_SIZE: usize = 16;
        alloc += align_up(n_clip as usize, CLIP_PART_SIZE as usize) * CLIP_BBOX_SIZE;
        let draw_bbox_alloc = alloc;
        alloc += n_drawobj * DRAW_BBOX_SIZE;
        let drawinfo_alloc = alloc;
        // TODO: not optimized; it can be accumulated during encoding or summed from drawtags
        const MAX_DRAWINFO_SIZE: usize = 44;
        alloc += n_drawobj * MAX_DRAWINFO_SIZE;

        let config = Config {
            n_elements: n_drawobj as u32,
            n_pathseg: self.n_pathseg,
            pathseg_alloc: pathseg_alloc as u32,
            anno_alloc: anno_alloc as u32,
            trans_alloc: trans_alloc as u32,
            path_bbox_alloc: path_bbox_alloc as u32,
            drawmonoid_alloc: drawmonoid_alloc as u32,
            clip_alloc: clip_alloc as u32,
            clip_bic_alloc: clip_bic_alloc as u32,
            clip_stack_alloc: clip_stack_alloc as u32,
            clip_bbox_alloc: clip_bbox_alloc as u32,
            draw_bbox_alloc: draw_bbox_alloc as u32,
            drawinfo_alloc: drawinfo_alloc as u32,
            n_trans: n_trans as u32,
            n_path: self.n_path,
            n_clip: self.n_clip,
            trans_offset: trans_offset as u32,
            linewidth_offset: linewidth_offset as u32,
            pathtag_offset: pathtag_offset as u32,
            pathseg_offset: pathseg_offset as u32,
            drawtag_offset: drawtag_offset as u32,
            drawdata_offset: drawdata_offset as u32,
            ..Default::default()
        };
        (config, alloc)
    }

    pub fn write_scene(&self, buf: &mut BufWrite) {
        buf.extend_slice(&self.drawtag_stream);
        let n_drawobj = self.drawtag_stream.len();
        buf.fill_zero(padding(n_drawobj, DRAW_PART_SIZE as usize) * DRAWTAG_SIZE);
        buf.extend_slice(&self.drawdata_stream);
        buf.extend_slice(&self.transform_stream);
        let n_trans = self.transform_stream.len();
        buf.fill_zero(padding(n_trans, TRANSFORM_PART_SIZE as usize) * TRANSFORM_SIZE);
        buf.extend_slice(&self.linewidth_stream);
        buf.extend_slice(&self.tag_stream);
        let n_pathtag = self.tag_stream.len();
        buf.fill_zero(padding(n_pathtag, PATHSEG_PART_SIZE as usize));
        buf.extend_slice(&self.pathseg_stream);
    }

    /// The number of draw objects in the draw object stream.
    pub(crate) fn n_drawobj(&self) -> usize {
        self.drawtag_stream.len()
    }

    /// The number of paths.
    pub(crate) fn n_path(&self) -> u32 {
        self.n_path
    }

    /// The number of path segments.
    pub(crate) fn n_pathseg(&self) -> u32 {
        self.n_pathseg
    }

    pub(crate) fn n_transform(&self) -> usize {
        self.transform_stream.len()
    }

    /// The number of tags in the path stream.
    pub(crate) fn n_pathtag(&self) -> usize {
        self.tag_stream.len()
    }

    pub(crate) fn n_clip(&self) -> u32 {
        self.n_clip
    }

    pub(crate) fn encode_glyph(&mut self, glyph: &GlyphEncoder) {
        self.tag_stream.extend(&glyph.tag_stream);
        self.pathseg_stream.extend(&glyph.pathseg_stream);
        self.drawtag_stream.extend(&glyph.drawtag_stream);
        self.drawdata_stream.extend(&glyph.drawdata_stream);
        self.n_path += glyph.n_path;
        self.n_pathseg += glyph.n_pathseg;
    }
}

fn align_up(x: usize, align: usize) -> usize {
    debug_assert!(align.is_power_of_two());
    (x + align - 1) & !(align - 1)
}

fn padding(x: usize, align: usize) -> usize {
    x.wrapping_neg() & (align - 1)
}

impl GlyphEncoder {
    pub fn path_encoder(&mut self) -> PathEncoder {
        PathEncoder::new(&mut self.tag_stream, &mut self.pathseg_stream)
    }

    pub fn finish_path(&mut self, n_pathseg: u32) {
        self.n_path += 1;
        self.n_pathseg += n_pathseg;
    }

    /// Encode a fill color draw object.
    ///
    /// This should be encoded after a path.
    pub(crate) fn fill_color(&mut self, rgba_color: u32) {
        self.drawtag_stream.push(DRAWTAG_FILLCOLOR);
        let element = FillColor { rgba_color };
        self.drawdata_stream.extend(bytemuck::bytes_of(&element));
    }

    pub(crate) fn is_color(&self) -> bool {
        !self.drawtag_stream.is_empty()
    }
}
