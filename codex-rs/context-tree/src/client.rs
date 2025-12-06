use crate::cache::ContextTreeCache;
use crate::recorder::HistoryMutationRecorder;
use crate::summarizer::ContextTreeSummarizer;
use crate::summarizer::PromptModel;

#[derive(Clone, Debug)]
pub struct ContextTreeClient<M: PromptModel> {
    summarizer: ContextTreeSummarizer<M>,
    cache: ContextTreeCache,
    recorder: HistoryMutationRecorder,
}

impl<M: PromptModel> ContextTreeClient<M> {
    pub fn new(model: Option<M>) -> Self {
        Self {
            summarizer: ContextTreeSummarizer::new(model),
            cache: ContextTreeCache::new(),
            recorder: HistoryMutationRecorder::new(0),
        }
    }

    pub fn recorder(&mut self) -> &mut HistoryMutationRecorder {
        &mut self.recorder
    }

    pub fn cache(&self) -> &ContextTreeCache {
        &self.cache
    }

    pub fn toggle_selected(&mut self, node_ids: &[String]) {
        self.cache.toggle_selected(node_ids);
    }

    pub fn toggle_collapsed(&mut self, node_ids: &[String]) {
        self.cache.toggle_collapsed(node_ids);
    }

    pub async fn commit_history_mutation(&mut self) {
        let mutations = self.recorder.commit();
        if mutations.is_empty() {
            return;
        }

        self.cache
            .apply_mutations(&self.summarizer, mutations)
            .await;
    }
}
