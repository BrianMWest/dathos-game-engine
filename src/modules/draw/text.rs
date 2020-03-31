use super::sprite::{
    DrawInstance, SpriteData, SpriteProgram, SpriteProgramBase, SpriteShaderInterface,
};
use super::{DrawCommand, ObjectGeometry, SAMPLER, SCREEN_HEIGHT, SCREEN_WIDTH};
use glyph_brush::rusttype::Scale;
use glyph_brush::{
    BrushAction, BrushError, GlyphBrush, GlyphBrushBuilder, GlyphVertex, HorizontalAlign, Layout,
    Section, VerticalAlign,
};
use luminance::context::GraphicsContext;
use luminance::framebuffer::Framebuffer;
use luminance::pipeline::{Pipeline, PipelineState, ShadingGate};
use luminance::pixel::{Depth32F, NormRGBA8UI};
use luminance::shader::program::Program;
use luminance::tess::Tess;
use luminance::texture::{Dim2, GenMipmaps, Texture};
use nalgebra::Vector2;

const MONTSERRAT_REGULAR: &[u8] = include_bytes!("./text/Montserrat-Regular.ttf");

type TextGeometry = (Vector2<f32>, Vector2<f32>, ObjectGeometry);

pub struct TextProgramBase<'a> {
    pub text: Texture<Dim2, NormRGBA8UI>,
    pub brush: GlyphBrush<'a, TextGeometry>,
    pub buffer: Framebuffer<Dim2, NormRGBA8UI, Depth32F>,
}

pub struct TextProgram<'a, 'b> {
    pub sprite: &'a SpriteProgramBase,
    pub text: &'a mut TextProgramBase<'b>,
    pub program: &'a Program<(), (), SpriteShaderInterface>,
    pub tess: &'a Tess,
}

impl<'a> TextProgramBase<'a> {
    pub fn new<C>(graphics_context: &mut C) -> Self
    where
        C: GraphicsContext,
    {
        TextProgramBase {
            text: Texture::<Dim2, NormRGBA8UI>::new(
                graphics_context,
                [SCREEN_WIDTH as u32, SCREEN_HEIGHT as u32],
                0,
                SAMPLER,
            )
            .unwrap(),
            brush: GlyphBrushBuilder::using_font_bytes(MONTSERRAT_REGULAR).build(),
            buffer: Framebuffer::new(
                graphics_context,
                [SCREEN_WIDTH as u32, SCREEN_HEIGHT as u32],
                0,
                SAMPLER,
            )
            .unwrap(),
        }
    }

    fn rasterize<C>(
        &mut self,
        graphics_context: &mut C,
    ) -> Result<BrushAction<TextGeometry>, BrushError>
    where
        C: GraphicsContext,
    {
        let brush = &mut self.brush;
        let text = &mut self.text;

        let brush_dimensions = brush.texture_dimensions();
        let cache_dimensions = text.size();
        if brush_dimensions != (cache_dimensions[0] - 32, cache_dimensions[1] - 32) {
            *text = Texture::<_, _>::new(
                graphics_context,
                [brush_dimensions.0 + 32, brush_dimensions.1 + 32],
                0,
                SAMPLER,
            )
            .unwrap();
        }

        brush.process_queued(
            |bounds, raster_bytes| {
                let _ = text.upload_part(
                    GenMipmaps::No,
                    [bounds.min.x, bounds.min.y],
                    [bounds.width(), bounds.height()],
                    &raster_bytes
                        .iter()
                        .map(|pixel| (255, 255, 255, *pixel))
                        .collect::<Vec<_>>(),
                );
            },
            |GlyphVertex {
                 tex_coords,
                 pixel_coords,
                 bounds: _,
                 color,
                 z,
             }| {
                (
                    Vector2::new(
                        brush_dimensions.0 as f32 * tex_coords.min.x as f32,
                        brush_dimensions.1 as f32 * tex_coords.min.y as f32,
                    ),
                    Vector2::new(
                        brush_dimensions.0 as f32 * tex_coords.width(),
                        brush_dimensions.1 as f32 * tex_coords.height(),
                    ),
                    ObjectGeometry {
                        darken: color.into(),
                        depth: z,
                        position: Into::<Vector2<f32>>::into([
                            (pixel_coords.min.x + pixel_coords.max.x) as f32,
                            (pixel_coords.min.y + pixel_coords.max.y) as f32,
                        ]) / 2.0,
                        ..Default::default()
                    },
                )
            },
        )
    }
}

impl<'a, 'b> TextProgram<'a, 'b> {
    pub fn prepare_render<C>(&mut self, graphics_context: &mut C, commands: &Vec<DrawCommand>)
    where
        C: GraphicsContext,
    {
        commands.iter().for_each(|command| {
            if let DrawCommand::Text {
                depth,
                color,
                position,
                size,
                text,
            } = command
            {
                self.text.brush.queue(Section {
                    color: *color,
                    layout: Layout::default_wrap()
                        .h_align(HorizontalAlign::Center)
                        .v_align(VerticalAlign::Center),
                    scale: Scale::uniform(*size),
                    screen_position: (position.x, -position.y),
                    text: &text,
                    z: *depth,
                    ..Default::default()
                });
            }
        });

        match self.text.rasterize(graphics_context) {
            Ok(BrushAction::Draw(vertices)) => {
                let pipeline_state = PipelineState::new()
                    .enable_clear_color(true)
                    .set_clear_color([0.0, 0.0, 0.0, 0.0]);
                graphics_context.pipeline_builder().pipeline(
                    &self.text.buffer,
                    &pipeline_state,
                    |pipeline, mut shading_gate| {
                        let image_size = self.text.text.size();
                        SpriteProgram {
                            base: self.sprite,
                            program: self.program,
                            tess: self.tess,
                            object: SpriteData::Override {
                                texture: &self.text.text,
                                depth_buffer: None,
                                instances: &vertices
                                    .iter()
                                    .map(|(offset, size, geometry)| DrawInstance {
                                        offset: Vector2::new(
                                            offset.x,
                                            image_size[1] as f32 - (offset.y + size.y),
                                        ),
                                        size: *size,
                                        geometry: geometry.clone(),
                                    })
                                    .collect::<Vec<DrawInstance>>(),
                            },
                        }
                        .render(&pipeline, &mut shading_gate);
                    },
                );
            }
            Ok(BrushAction::ReDraw) => {
                // No need to do anything if the framebuffer is already valid
            }
            Err(BrushError::TextureTooSmall { suggested }) => {
                self.text.brush.resize_texture(suggested.0, suggested.1);
                self.prepare_render(graphics_context, commands);
            }
        };
    }

    pub fn render<C>(&mut self, pipeline: &Pipeline, shading_gate: &mut ShadingGate<C>)
    where
        C: GraphicsContext,
    {
        let image_size = self.text.buffer.color_slot().size();
        SpriteProgram {
            base: self.sprite,
            program: self.program,
            tess: self.tess,
            object: SpriteData::Override {
                texture: &self.text.buffer.color_slot(),
                depth_buffer: Some(&self.text.buffer.depth_slot()),
                instances: &vec![DrawInstance {
                    offset: [0.0, 0.0].into(),
                    size: [image_size[0] as f32, image_size[1] as f32].into(),
                    geometry: ObjectGeometry {
                        depth: 100.0,
                        scale: [1.0, -1.0].into(),
                        ..Default::default()
                    },
                }],
            },
        }
        .render(pipeline, shading_gate);
    }
}