use crate::host::State;
use crate::plugins::main;
use crate::plugins::main::types::{Embedding, EmbeddingModel, EmbeddingModelType};
use crate::resource::Resource;
use crate::resource::ResourceStorage;

use kalosm::language::*;
use kalosm_common::ModelLoadingProgress;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;

pub(crate) enum LazyTextEmbeddingModel {
    Uninitialized(EmbeddingModelType),
    Bert(Arc<Bert>),
}

impl LazyTextEmbeddingModel {
    fn initialize(
        &self,
    ) -> impl std::future::Future<Output = anyhow::Result<Bert>> + Send + Sync + 'static {
        let embedding_model_type = match self {
            LazyTextEmbeddingModel::Uninitialized(ty) => Some(ty.clone()),
            _ => None,
        };
        async move {
            let ty = embedding_model_type.ok_or(anyhow::anyhow!("Model already initialized"))?;
            match ty {
                main::types::EmbeddingModelType::Bert => {
                    let model = Bert::builder()
                        .build_with_loading_handler(move |progress: ModelLoadingProgress| {
                            if let Some(callbacks) = EMBEDDING_MODEL_DOWNLOAD_PROGRESS
                                .write()
                                .unwrap()
                                .get_mut(&(ty as usize))
                            {
                                for callback in callbacks {
                                    callback(progress.clone());
                                }
                            }
                        })
                        .await?;
                    Ok(model)
                }
            }
        }
    }

    fn value(&self) -> Option<Arc<Bert>> {
        match self {
            LazyTextEmbeddingModel::Bert(model) => Some(model.clone()),
            _ => None,
        }
    }
}

#[allow(clippy::type_complexity)]
static EMBEDDING_MODEL_DOWNLOAD_PROGRESS: Lazy<
    RwLock<HashMap<usize, Vec<Box<dyn FnMut(ModelLoadingProgress) + Send + Sync>>>>,
> = Lazy::new(Default::default);

pub fn listen_to_embedding_model_download_progresses<
    F: FnMut(ModelLoadingProgress) + Send + Sync + 'static,
>(
    model_type: main::types::EmbeddingModelType,
    f: F,
) {
    let mut progress = EMBEDDING_MODEL_DOWNLOAD_PROGRESS.write().unwrap();
    let model_type_as_id = model_type as usize;
    progress
        .entry(model_type_as_id)
        .or_default()
        .push(Box::new(f));
}

impl main::types::EmbeddingModelType {
    /// Returns whether the model has been downloaded.
    pub fn model_downloaded_sync(&self) -> bool {
        !Bert::builder().requires_download()
    }
}

impl State {
    async fn initialize_text_embedding_model(
        &mut self,
        index: Resource<LazyTextEmbeddingModel>,
    ) -> wasmtime::Result<Arc<Bert>> {
        let raw_index = index.into();
        {
            let future = {
                let borrow = self
                    .resources
                    .get_mut(raw_index)
                    .ok_or(anyhow::anyhow!("Text Embedding Model not found"))?;
                match &*borrow {
                    LazyTextEmbeddingModel::Uninitialized(_) => Some(borrow.initialize()),
                    _ => None,
                }
            };
            if let Some(fut) = future {
                let model = fut.await?;
                let mut borrow = self
                    .resources
                    .get_mut(raw_index)
                    .ok_or(anyhow::anyhow!("Text Embedding Model not found"))?;
                *borrow = LazyTextEmbeddingModel::Bert(Arc::new(model));
            }
        }
        let borrow = self
            .resources
            .get_mut(raw_index)
            .ok_or(anyhow::anyhow!("Text Embedding Model not found"))?;
        Ok(borrow.value().unwrap())
    }

    pub(crate) fn impl_create_embedding_model(
        &mut self,
        ty: main::types::EmbeddingModelType,
    ) -> wasmtime::Result<EmbeddingModel> {
        let model = LazyTextEmbeddingModel::Uninitialized(ty);
        let idx = self.resources.insert(model);

        Ok(EmbeddingModel {
            id: idx.index() as u64,
            owned: true,
        })
    }

    pub(crate) async fn impl_embedding_model_downloaded(
        &mut self,
        ty: main::types::EmbeddingModelType,
    ) -> wasmtime::Result<bool> {
        Ok(ty.model_downloaded_sync())
    }

    pub(crate) async fn impl_get_embedding(
        &mut self,
        self_: EmbeddingModel,
        document: String,
    ) -> wasmtime::Result<Embedding> {
        let index = self_.into();
        let model = self.initialize_text_embedding_model(index).await?;
        Ok(main::types::Embedding {
            vector: model.embed(&document).await?.to_vec(),
        })
    }

    pub(crate) fn impl_drop_embedding_model(
        &mut self,
        rep: EmbeddingModel,
    ) -> wasmtime::Result<()> {
        let index = rep.into();
        self.resources.drop_key(index);
        Ok(())
    }
}
