use crate::io::pipeline::Processable;
use crate::{
    context::MapContext,
    coords::{WorldCoords, WorldTileCoords, Zoom, TILE_SIZE},
    environment::Kernel,
    error::Error,
    headless::{
        environment::HeadlessEnvironment, graph_node::CopySurfaceBufferNode,
        stage::WriteSurfaceBufferStage,
    },
    io::{
        pipeline::{PipelineContext, PipelineProcessor},
        tile_pipelines::build_vector_tile_pipeline,
        tile_repository::{StoredLayer, StoredTile, TileStatus},
        RawLayer, TileRequest,
    },
    render::{
        create_default_render_graph, draw_graph, error::RenderError, eventually::Eventually,
        register_default_render_stages, resource::Head, stages::RenderStageLabel, Renderer,
        ShaderVertex,
    },
    schedule::{Schedule, Stage},
    style::Style,
    tessellation::{IndexDataType, OverAlignedVertexBuffer},
    window::WindowSize,
    world::World,
};
use std::collections::HashSet;

pub struct HeadlessMap {
    window_size: WindowSize,
    kernel: Kernel<HeadlessEnvironment>,
    map_context: MapContext,
    schedule: Schedule,
}

impl HeadlessMap {
    pub fn new(
        style: Style,
        window_size: WindowSize,
        renderer: Renderer,
        kernel: Kernel<HeadlessEnvironment>,
    ) -> Result<Self, Error> {
        let world = World::new(
            window_size,
            WorldCoords::from((TILE_SIZE / 2., TILE_SIZE / 2.)),
            Zoom::default(),
            cgmath::Deg(110.0),
        );

        let mut schedule = Schedule::default();

        let mut graph = create_default_render_graph()?;
        let draw_graph = graph
            .get_sub_graph_mut(draw_graph::NAME)
            .expect("Subgraph does not exist");
        draw_graph.add_node(draw_graph::node::COPY, CopySurfaceBufferNode::default());
        draw_graph
            .add_node_edge(draw_graph::node::MAIN_PASS, draw_graph::node::COPY)
            .unwrap(); // TODO: remove unwrap

        register_default_render_stages(graph, &mut schedule);

        schedule.add_stage(
            RenderStageLabel::Cleanup,
            WriteSurfaceBufferStage::default(),
        );

        Ok(Self {
            window_size,
            kernel,
            map_context: MapContext {
                style,
                world,
                renderer,
            },
            schedule,
        })
    }

    pub async fn render_tile(&mut self, tile: StoredTile) -> Result<(), Error> {
        let context = &mut self.map_context;

        if let Eventually::Initialized(pool) = context.renderer.state.buffer_pool_mut() {
            pool.clear();
        }

        context.world.tile_repository.put_tile(tile);

        self.schedule.run(&mut self.map_context);
        Ok(())
    }

    pub async fn fetch_tile(
        &self,
        coords: WorldTileCoords,
        source_layers: HashSet<String>,
    ) -> Result<StoredTile, Error> {
        let source_client = &self.kernel.source_client;

        let data = source_client.fetch(&coords).await?.into_boxed_slice();

        let mut pipeline_context = PipelineContext::new(HeadlessPipelineProcessor::default());
        let pipeline = build_vector_tile_pipeline();

        pipeline.process(
            (
                TileRequest {
                    coords: WorldTileCoords::default(),
                    layers: source_layers,
                },
                data,
            ),
            &mut pipeline_context,
        );

        let mut processor = pipeline_context
            .take_processor::<HeadlessPipelineProcessor>()
            .expect("Unable to get processor");

        Ok(StoredTile::success(coords, processor.layers))
    }
}

#[derive(Default)]
pub struct HeadlessPipelineProcessor {
    pub layers: Vec<StoredLayer>,
}

impl PipelineProcessor for HeadlessPipelineProcessor {
    fn layer_tesselation_finished(
        &mut self,
        coords: &WorldTileCoords,
        buffer: OverAlignedVertexBuffer<ShaderVertex, IndexDataType>,
        feature_indices: Vec<u32>,
        layer_data: RawLayer,
    ) {
        self.layers.push(StoredLayer::TessellatedLayer {
            coords: *coords,
            layer_name: layer_data.name,
            buffer,
            feature_indices,
        })
    }
}
