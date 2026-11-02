import * as vscode from 'vscode';

/** The Devscriptor Analysis view: one row per command, always the same. */
export class AnalysisProvider implements vscode.TreeDataProvider<AnalysisItem> {
    getTreeItem(element: AnalysisItem): vscode.TreeItem {
        return element;
    }

    getChildren(element?: AnalysisItem): Thenable<AnalysisItem[]> {
        if (!element) {
            return Promise.resolve([
                new AnalysisItem(
                    'Scan Workspace',
                    vscode.TreeItemCollapsibleState.None,
                    {
                        command: 'devscriptor.scanWorkspace',
                        title: 'Scan Workspace'
                    },
                    '$(refresh)'
                ),
                new AnalysisItem(
                    'Search',
                    vscode.TreeItemCollapsibleState.None,
                    {
                        command: 'devscriptor.semanticSearch',
                        title: 'Search'
                    },
                    '$(search)'
                ),
                new AnalysisItem(
                    'Find Dead Code',
                    vscode.TreeItemCollapsibleState.None,
                    {
                        command: 'devscriptor.findDeadCode',
                        title: 'Find Dead Code'
                    },
                    '$(trash)'
                ),
                new AnalysisItem(
                    'Analyze Complexity',
                    vscode.TreeItemCollapsibleState.None,
                    {
                        command: 'devscriptor.analyzeComplexity',
                        title: 'Analyze Complexity'
                    },
                    '$(graph-line)'
                ),
                new AnalysisItem(
                    'Show Call Graph',
                    vscode.TreeItemCollapsibleState.None,
                    {
                        command: 'devscriptor.showCallGraph',
                        title: 'Show Call Graph'
                    },
                    '$(git-branch)'
                ),
                new AnalysisItem(
                    'Connect to the Shared Devscriptor',
                    vscode.TreeItemCollapsibleState.None,
                    {
                        command: 'devscriptor.startMcpServer',
                        title: 'Connect to the Shared Devscriptor'
                    },
                    '$(server)'
                )
            ]);
        }

        return Promise.resolve([]);
    }
}

class AnalysisItem extends vscode.TreeItem {
    constructor(
        public readonly label: string,
        public readonly collapsibleState: vscode.TreeItemCollapsibleState,
        public readonly command?: vscode.Command,
        public readonly icon?: string
    ) {
        super(label, collapsibleState);
        this.tooltip = this.label;
        if (this.icon) {
            this.iconPath = new vscode.ThemeIcon(this.icon.replace('$(', '').replace(')', ''));
        }
    }
}
