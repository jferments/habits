// Habits browser journal: top-level URL/title metadata and active-tab attention.
// No content scripts, page bodies, DOM inspection, or network upload.
// The extension/bridge are optional: their absence never affects habagent.exe.
'use strict';
const HOST_NAME = 'org.jferments.habits';
const BROWSER_NAME = chrome.runtime.getURL('').startsWith('moz-extension:')
    ? 'firefox' : 'edge';
const MAX_QUEUED = 20000;
const state = { queue:[], ready:false, saving:false, dirty:false,
    inflight:null, port:null, reconnect:null, lastFocus:null,
    lastPulse:0, lastVisits:new Map(), lastTitles:new Map(), overflow:false };

function safePageUrl(rawUrl) {
    try {
        const parsed = new URL(rawUrl);
        if (parsed.protocol !== 'https:' && parsed.protocol !== 'http:') return null;
        // Strip URL-embedded credentials, but preserve paths, queries and hashes:
        // they can be part of the page identity (and may contain sensitive data).
        parsed.username='';
        parsed.password='';
        const value=parsed.toString();
        return value.length <= 16384 ? value : null;
    } catch (_) { return null; }
}

function hostOf(rawUrl) {
    try {
        const parsed = new URL(rawUrl);
        if (parsed.protocol !== 'https:' && parsed.protocol !== 'http:') return null;
        return parsed.hostname.toLowerCase().replace(/\.$/, '') || null;
    } catch (_) { return null; }
}

function eventId() {
    if (globalThis.crypto && typeof globalThis.crypto.randomUUID === 'function') {
        return globalThis.crypto.randomUUID();
    }
    return String(Date.now()) + '-' + Math.random().toString(36).slice(2);
}

function enqueue(kind, host, tab, metadata=null) {
    if (kind !== 'blur' && !host) return;
    const item = {
        id:eventId(), browser:BROWSER_NAME, kind,
        host:host || null, timestamp_ms:Date.now(),
        tab_id:tab && typeof tab.id === 'number' ? tab.id : null,
        window_id:tab && typeof tab.windowId === 'number' ? tab.windowId : null
    };
    if (metadata && (kind === 'visit' || kind === 'title')) {
        item.url=metadata.url;
        if (metadata.title !== undefined) item.title=metadata.title;
    }
    state.queue.push(item);
    if (state.queue.length > MAX_QUEUED) {
        // A disconnected host must not grow storage without bound. Keep the
        // most recent events and make lost coverage visible to the user.
        state.queue.splice(0, state.queue.length - MAX_QUEUED);
        state.overflow = true;
        chrome.action.setBadgeText({text:'!'});
        chrome.action.setBadgeBackgroundColor({color:'#b42318'});
    }
    saveQueue();
}

function saveQueue() {
    if (!state.ready) return;
    if (state.saving) { state.dirty=true; return; }
    state.saving=true;
    chrome.storage.local.set({pending:state.queue,queueOverflow:state.overflow}, () => {
        state.saving=false;
        if (chrome.runtime.lastError) {
            console.warn('Habits: could not persist visit queue:',chrome.runtime.lastError.message);
            return;
        }
        if (state.dirty) { state.dirty=false; saveQueue(); return; }
        sendNext();
    });
}

function connectNative() {
    if (!state.ready || state.port) return;
    try {
        const port = chrome.runtime.connectNative(HOST_NAME);
        state.port=port;
        port.onMessage.addListener(reply => {
            const current = state.inflight;
            if (!current || !reply || reply.id !== current) return;
            if (reply.ok === false) console.warn('Habits rejected an invalid event:',reply.error);
            state.inflight=null;
            if (state.queue.length && state.queue[0].id === current) state.queue.shift();
            saveQueue();
        });
        port.onDisconnect.addListener(() => {
            const err=chrome.runtime.lastError;
            if (err) console.warn('Habits native bridge unavailable:',err.message);
            if (state.port === port) state.port=null;
            state.inflight=null;
            if (!state.reconnect) {
                state.reconnect=setTimeout(() => {
                    state.reconnect=null;
                    connectNative();
                },15000);
            }
        });
        sendNext();
    } catch (error) {
        console.warn('Habits: native messaging unavailable:',error);
    }
}

function sendNext() {
    if (!state.ready || state.saving || state.inflight || !state.queue.length) return;
    if (!state.port) { connectNative(); return; }
    const next=state.queue[0];
    state.inflight=next.id;
    try { state.port.postMessage(next); }
    catch (error) {
        state.inflight=null;
        console.warn('Habits: could not send event:',error);
    }
}

function recordVisit(details) {
    if (typeof details.tabId !== 'number' || details.tabId < 0 ||
        (typeof details.frameId === 'number' && details.frameId !== 0)) return;
    const pageUrl=safePageUrl(details.url);
    if (!pageUrl) return;
    const host=hostOf(pageUrl);
    if (!host) return;
    const key=details.tabId + ':' + pageUrl;
    const now=Date.now();
    // Prevent duplicate navigation events for a single redirect/commit.
    if (now - (state.lastVisits.get(key) || 0) < 250) return;
    state.lastVisits.set(key,now);
    if (state.lastVisits.size > 1024) state.lastVisits.clear();

    chrome.tabs.get(details.tabId,tab => {
        if (chrome.runtime.lastError || !tab || tab.incognito) return;
        // A tab title may already be available at commit time. Only trust it
        // when the tab URL actually matches this navigation (not the prior page).
        const title = safePageUrl(tab.url) === pageUrl && typeof tab.title === 'string'
            ? tab.title.replace(/[\u0000-\u001f\u007f]/g, '').slice(0,2048)
            : undefined;
        enqueue('visit',host,tab,{url:pageUrl,title});
        if (tab.active) syncAttention();
    });
}

function recordTitle(tab) {
    if (!tab || tab.incognito || typeof tab.title !== 'string' || !tab.title) return;
    const pageUrl=safePageUrl(tab.url);
    const host=pageUrl && hostOf(pageUrl);
    if (!host) return;
    const title=tab.title.replace(/[\u0000-\u001f\u007f]/g, '').slice(0,2048);
    if (!title) return;
    const key=tab.id + ':' + pageUrl + ':' + title;
    if (state.lastTitles.has(key)) return;
    state.lastTitles.set(key,Date.now());
    if (state.lastTitles.size > 1024) state.lastTitles.clear();
    enqueue('title',host,tab,{url:pageUrl,title});
}

function blur() {
    if (state.lastFocus) {
        enqueue('blur',null,null);
        state.lastFocus=null;
        state.lastPulse=0;
    }
}

function syncAttention() {
    chrome.windows.getLastFocused({populate:true}, win => {
        if (chrome.runtime.lastError || !win || !win.focused) { blur(); return; }
        const tab=(win.tabs || []).find(t => t.active);
        if (!tab || tab.incognito) { blur(); return; }
        const host=hostOf(tab.url || tab.pendingUrl);
        if (!host) { blur(); return; }
        chrome.idle.queryState(90, idle => {
            if (chrome.runtime.lastError || idle !== 'active') { blur(); return; }
            // Don't report a tab unless the correct window was still focused.
            chrome.windows.get(win.id,{populate:false}, fresh => {
                if (chrome.runtime.lastError || !fresh || !fresh.focused) { blur(); return; }
                const focusKey=win.id + ':' + tab.id + ':' + host;
                const now=Date.now();
                if (focusKey !== state.lastFocus) {
                    state.lastFocus=focusKey;
                    state.lastPulse=now;
                    enqueue('focus',host,tab);
                } else if (now - state.lastPulse >= 14000) {
                    state.lastPulse=now;
                    enqueue('heartbeat',host,tab);
                }
            });
        });
    });
}

// Event registrations must be synchronous for an Edge MV3 service worker.
chrome.webNavigation.onCommitted.addListener(recordVisit);
chrome.webNavigation.onHistoryStateUpdated.addListener(recordVisit);
chrome.webNavigation.onReferenceFragmentUpdated.addListener(recordVisit);
chrome.tabs.onActivated.addListener(syncAttention);
chrome.tabs.onUpdated.addListener((id, change, tab) => {
    if (change.title || change.status === 'complete') recordTitle(tab);
    if (change.url || change.status === 'complete') syncAttention();
});
chrome.tabs.onRemoved.addListener(syncAttention);
chrome.windows.onFocusChanged.addListener(syncAttention);
chrome.idle.onStateChanged.addListener(syncAttention);
chrome.runtime.onStartup.addListener(syncAttention);
chrome.runtime.onInstalled.addListener(syncAttention);
chrome.alarms.onAlarm.addListener(alarm => {
    if (alarm.name === 'habits-pulse') {
        syncAttention();
        if (!state.port) connectNative();
    }
});

chrome.storage.local.get({pending:[],queueOverflow:false}, result => {
    if (chrome.runtime.lastError) {
        console.warn('Habits: failed to load offline queue:',chrome.runtime.lastError.message);
        return;
    }
    // Never overwrite newly queued events that arrived during storage loading.
    state.queue=(Array.isArray(result.pending)?result.pending:[]).concat(state.queue);
    state.overflow=!!result.queueOverflow;
    state.ready=true;
    if (state.overflow) chrome.action.setBadgeText({text:'!'});
    chrome.alarms.create('habits-pulse',{periodInMinutes:0.5});
    if (state.queue.length) saveQueue();
    else connectNative();
    syncAttention();
});
