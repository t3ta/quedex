// Quedex Dashboard Application
class QuedexDashboard {
    constructor() {
        this.ws = null;
        this.runs = [];
        this.currentRunId = null;
        this.currentTaskId = null;
        this.logsTab = 'stdout';
        
        this.init();
    }

    init() {
        this.connectWebSocket();
        this.bindEvents();
        this.fetchInitialState();
    }

    connectWebSocket() {
        const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
        const wsUrl = `${protocol}//${window.location.host}/ws`;
        
        this.ws = new WebSocket(wsUrl);
        
        this.ws.onopen = () => {
            this.updateConnectionStatus(true);
        };
        
        this.ws.onclose = () => {
            this.updateConnectionStatus(false);
            // Reconnect after 3 seconds
            setTimeout(() => this.connectWebSocket(), 3000);
        };
        
        this.ws.onerror = () => {
            this.updateConnectionStatus(false);
        };
        
        this.ws.onmessage = (event) => {
            try {
                const data = JSON.parse(event.data);
                this.handleStateUpdate(data);
            } catch (e) {
                console.error('Failed to parse WebSocket message:', e);
            }
        };
    }

    updateConnectionStatus(connected) {
        const el = document.getElementById('connection-status');
        el.textContent = connected ? 'Connected' : 'Disconnected';
        el.className = connected ? 'status-connected' : 'status-disconnected';
    }

    async fetchInitialState() {
        try {
            const response = await fetch('/api/state');
            const data = await response.json();
            this.handleStateUpdate(data);
        } catch (e) {
            console.error('Failed to fetch initial state:', e);
        }
    }

    handleStateUpdate(data) {
        if (data.runs) {
            this.runs = data.runs;
            this.renderRuns();
            
            if (this.currentRunId) {
                this.renderTasks();
            }
        } else if (data.run_id) {
            // Single run update
            const index = this.runs.findIndex(r => r.run_id === data.run_id);
            if (index >= 0) {
                this.runs[index] = data;
            } else {
                this.runs.unshift(data);
            }
            this.renderRuns();
            
            if (this.currentRunId === data.run_id) {
                this.renderTasks();
            }
        }
    }

    bindEvents() {
        document.getElementById('back-btn').addEventListener('click', () => {
            this.showRunsList();
        });
        
        document.getElementById('close-logs-btn').addEventListener('click', () => {
            this.hideLogs();
        });
        
        document.querySelectorAll('.tab-btn').forEach(btn => {
            btn.addEventListener('click', (e) => {
                this.switchLogsTab(e.target.dataset.tab);
            });
        });
    }

    renderRuns() {
        const container = document.getElementById('runs-list');
        
        if (this.runs.length === 0) {
            container.innerHTML = '<p style="color: var(--text-secondary)">No runs found.</p>';
            return;
        }
        
        container.innerHTML = this.runs.map(run => `
            <div class="run-card" data-run-id="${run.run_id}">
                <h3>${this.escapeHtml(run.run_name || run.run_id)}</h3>
                <div class="run-id">${run.run_id}</div>
                <div class="run-status status-${run.status.toLowerCase()}">${run.status}</div>
                <div style="margin-top: 0.5rem; font-size: 0.875rem; color: var(--text-secondary)">
                    ${this.formatDate(run.started_at)}
                </div>
            </div>
        `).join('');
        
        container.querySelectorAll('.run-card').forEach(card => {
            card.addEventListener('click', () => {
                this.selectRun(card.dataset.runId);
            });
        });
    }

    selectRun(runId) {
        this.currentRunId = runId;
        document.getElementById('runs-container').style.display = 'none';
        document.getElementById('tasks-container').style.display = 'block';
        this.renderTasks();
    }

    showRunsList() {
        this.currentRunId = null;
        document.getElementById('runs-container').style.display = 'block';
        document.getElementById('tasks-container').style.display = 'none';
        this.hideLogs();
    }

    renderTasks() {
        const run = this.runs.find(r => r.run_id === this.currentRunId);
        if (!run) return;
        
        document.getElementById('current-run-name').textContent = run.run_name || run.run_id;
        
        // Render summary
        const tasks = Object.entries(run.tasks || {});
        const summary = {
            pending: 0, ready: 0, running: 0,
            succeeded: 0, failed: 0, canceled: 0, skipped: 0
        };
        
        tasks.forEach(([_, task]) => {
            const status = task.status.toLowerCase();
            if (summary[status] !== undefined) summary[status]++;
        });
        
        document.getElementById('tasks-summary').innerHTML = `
            <div class="summary-item"><span class="summary-count">${summary.succeeded}</span> Succeeded</div>
            <div class="summary-item"><span class="summary-count">${summary.failed}</span> Failed</div>
            <div class="summary-item"><span class="summary-count">${summary.running}</span> Running</div>
            <div class="summary-item"><span class="summary-count">${summary.pending + summary.ready}</span> Pending</div>
        `;
        
        // Render task list
        const container = document.getElementById('tasks-list');
        container.innerHTML = tasks.map(([taskId, task]) => `
            <div class="task-row" data-task-id="${taskId}">
                <div class="task-status ${task.status.toLowerCase()}"></div>
                <div class="task-info">
                    <div class="task-id">${this.escapeHtml(taskId)}</div>
                    <div class="task-title">${task.status}${task.exit_code !== null ? ` (exit: ${task.exit_code})` : ''}</div>
                </div>
                <div class="task-actions">
                    <button class="btn view-logs-btn">Logs</button>
                    ${task.status === 'Failed' || task.status === 'Canceled' ? 
                        `<button class="btn btn-warning retry-btn">Retry</button>` : ''}
                    ${task.status === 'Running' ? 
                        `<button class="btn btn-danger cancel-btn">Cancel</button>` : ''}
                </div>
            </div>
        `).join('');
        
        // Bind task actions
        container.querySelectorAll('.task-row').forEach(row => {
            const taskId = row.dataset.taskId;
            
            row.querySelector('.view-logs-btn')?.addEventListener('click', (e) => {
                e.stopPropagation();
                this.showLogs(taskId);
            });
            
            row.querySelector('.retry-btn')?.addEventListener('click', (e) => {
                e.stopPropagation();
                this.retryTask(taskId);
            });
            
            row.querySelector('.cancel-btn')?.addEventListener('click', (e) => {
                e.stopPropagation();
                this.cancelTask(taskId);
            });
        });
    }

    async showLogs(taskId) {
        this.currentTaskId = taskId;
        document.getElementById('current-task-id').textContent = taskId;
        document.getElementById('logs-container').style.display = 'block';
        
        try {
            const response = await fetch(`/api/logs/${this.currentRunId}/${taskId}`);
            const data = await response.json();
            this.logsData = data;
            this.renderLogs();
        } catch (e) {
            console.error('Failed to fetch logs:', e);
            document.getElementById('logs-content').textContent = 'Failed to load logs.';
        }
    }

    hideLogs() {
        this.currentTaskId = null;
        document.getElementById('logs-container').style.display = 'none';
    }

    switchLogsTab(tab) {
        this.logsTab = tab;
        document.querySelectorAll('.tab-btn').forEach(btn => {
            btn.classList.toggle('active', btn.dataset.tab === tab);
        });
        this.renderLogs();
    }

    renderLogs() {
        if (!this.logsData) return;
        const content = this.logsTab === 'stdout' ? this.logsData.stdout : this.logsData.stderr;
        document.getElementById('logs-content').textContent = content || '(empty)';
    }

    async retryTask(taskId) {
        try {
            await fetch(`/api/retry/${this.currentRunId}/${taskId}`, { method: 'POST' });
            // State will be updated via WebSocket
        } catch (e) {
            console.error('Failed to retry task:', e);
        }
    }

    async cancelTask(taskId) {
        try {
            await fetch(`/api/cancel/${this.currentRunId}/${taskId}`, { method: 'POST' });
            // State will be updated via WebSocket
        } catch (e) {
            console.error('Failed to cancel task:', e);
        }
    }

    escapeHtml(text) {
        const div = document.createElement('div');
        div.textContent = text;
        return div.innerHTML;
    }

    formatDate(dateStr) {
        if (!dateStr) return '';
        try {
            return new Date(dateStr).toLocaleString();
        } catch {
            return dateStr;
        }
    }
}

// Initialize dashboard
document.addEventListener('DOMContentLoaded', () => {
    window.dashboard = new QuedexDashboard();
});
