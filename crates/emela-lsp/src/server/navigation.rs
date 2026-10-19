//! Navigation requests. Folding and selection never invoke the checking frontend.

use super::*;

impl<Fs, M, F> Server<'_, Fs, M>
where
    Fs: FileSystem,
    M: FnMut() -> F,
    F: Frontend,
{
    pub(super) fn document_highlights(
        &mut self,
        params: &lsp::DocumentHighlightParams,
    ) -> Vec<lsp::DocumentHighlight> {
        let Some(at) = self.check_at(&params.text_document_position_params) else {
            return Vec::new();
        };
        let index = at.analysis.sources[at.file].line_index();
        F::document_highlights(&at.program, &at.analysis, at.file, at.offset)
            .into_iter()
            .map(|range| lsp::DocumentHighlight {
                range: convert::range(index, range),
                kind: Some(lsp::DocumentHighlightKind::TEXT),
            })
            .collect()
    }

    pub(super) fn type_definition(
        &mut self,
        params: &lsp::GotoDefinitionParams,
    ) -> Option<lsp::GotoDefinitionResponse> {
        let at = self.check_at(&params.text_document_position_params)?;
        let target = F::type_definition(&at.program, &at.analysis, at.file, at.offset)?;
        Some(lsp::GotoDefinitionResponse::Scalar(
            self.location(&at.analysis, target)?,
        ))
    }

    pub(super) fn implementations(
        &mut self,
        params: &lsp::GotoDefinitionParams,
    ) -> Vec<lsp::Location> {
        let Some(at) = self.check_at(&params.text_document_position_params) else {
            return Vec::new();
        };
        F::implementations(&at.program, &at.analysis, at.file, at.offset)
            .into_iter()
            .filter_map(|target| self.location(&at.analysis, target))
            .collect()
    }

    pub(super) fn workspace_symbols(
        &mut self,
        params: &lsp::WorkspaceSymbolParams,
    ) -> Vec<lsp::SymbolInformation> {
        let mut inputs = Vec::new();
        let mut paths: Vec<_> = self
            .workspace
            .iter()
            .map(|f| f.path.clone())
            .chain(self.documents.values().map(|d| d.path.clone()))
            .collect();
        paths.sort();
        for path in paths {
            let Ok(input) = Input::resolve(&self.fs, &path) else {
                continue;
            };
            let input = if input.single_file {
                Some(input)
            } else {
                Input::resolve(&self.fs, &input.base).ok()
            };
            if let Some(input) = input
                && !inputs.contains(&input)
            {
                inputs.push(input);
            }
        }
        let mut found = Vec::new();
        for input in inputs {
            let (input, _) = with_dependencies(&self.fs, input);
            let (analysis, _) = emela_driver::check(&self.fs, &input, &mut (self.make_frontend)());
            for symbol in emela_driver::workspace_symbols(&analysis, &params.query) {
                let Some(location) = self.location(&analysis, symbol.target) else {
                    continue;
                };
                #[allow(deprecated)]
                found.push(lsp::SymbolInformation {
                    name: symbol.name,
                    kind: convert::symbol_kind(symbol.kind),
                    tags: None,
                    deprecated: None,
                    location,
                    container_name: Some(symbol.container),
                });
            }
        }
        found.sort_by_key(|s| {
            (
                s.location.uri.clone(),
                s.location.range.start.line,
                s.location.range.start.character,
                s.name.clone(),
            )
        });
        found.dedup_by(|a, b| a.location == b.location && a.name == b.name);
        found
    }

    pub(super) fn folding_ranges(
        &self,
        params: &lsp::FoldingRangeParams,
    ) -> Option<Vec<lsp::FoldingRange>> {
        let text = self
            .fs
            .read_to_string(&self.path_of(&params.text_document.uri)?)
            .ok()?;
        let index = line_index::LineIndex::new(&text);
        Some(
            emela_driver::folding_ranges(&text)
                .into_iter()
                .map(|fold| {
                    let range = convert::range(&index, fold.range);
                    lsp::FoldingRange {
                        start_line: range.start.line,
                        start_character: Some(range.start.character),
                        end_line: range.end.line,
                        end_character: Some(range.end.character),
                        kind: match fold.kind {
                            emela_driver::FoldKind::Region => Some(lsp::FoldingRangeKind::Region),
                            emela_driver::FoldKind::Imports => Some(lsp::FoldingRangeKind::Imports),
                            emela_driver::FoldKind::Comment => Some(lsp::FoldingRangeKind::Comment),
                        },
                        collapsed_text: None,
                    }
                })
                .collect(),
        )
    }

    pub(super) fn selection_ranges(
        &self,
        params: &lsp::SelectionRangeParams,
    ) -> Option<Vec<lsp::SelectionRange>> {
        let text = self
            .fs
            .read_to_string(&self.path_of(&params.text_document.uri)?)
            .ok()?;
        let index = line_index::LineIndex::new(&text);
        let offsets: Option<Vec<_>> = params
            .positions
            .iter()
            .map(|&p| convert::offset(&index, p))
            .collect();
        emela_driver::selection_ranges(&text, &offsets?)
            .into_iter()
            .map(|ranges| {
                let mut parent = None;
                for range in ranges.into_iter().rev() {
                    parent = Some(Box::new(lsp::SelectionRange {
                        range: convert::range(&index, range),
                        parent,
                    }));
                }
                parent.map(|range| *range)
            })
            .collect()
    }
}
