// Node-only test of the browser metadata emitter. Requires no browser or native host.
// Both browsers use byte-identical background.js; run for each origin.
'use strict';
const assert=require('node:assert/strict');
const fs=require('node:fs');
const vm=require('node:vm');
const path=require('node:path');

function run(browserName) {
    const listeners={};
    const stored=[];
    const events=[];
    let tab={
        id:7, windowId:4, active:true, incognito:false,
        title:'A Page Title',
        url:'https://alice:password@example.com/path?q=hello#section'
    };
    const hook=name=>({addListener:fn=>{listeners[name]=fn;}});
    const chrome={
        runtime:{
            getURL:()=>browserName==='firefox'?'moz-extension://test/':'chrome-extension://test/',
            connectNative:()=>{
                // A broken/missing native host must never disable local queueing.
                throw new Error('Host is intentionally unavailable');
            },
            onStartup:hook('startup'),onInstalled:hook('installed'),lastError:null
        },
        webNavigation:{
            onCommitted:hook('committed'),
            onHistoryStateUpdated:hook('history'),
            onReferenceFragmentUpdated:hook('fragment')
        },
        tabs:{
            get:(id,cb)=>cb(tab),
            onActivated:hook('activated'),onUpdated:hook('updated'),onRemoved:hook('removed')
        },
        windows:{
            getLastFocused:(opts,cb)=>cb({focused:false}),
            get:(id,opts,cb)=>cb({focused:false}),
            onFocusChanged:hook('focuschanged')
        },
        idle:{
            queryState:(seconds,cb)=>cb('active'),
            onStateChanged:hook('idlechanged')
        },
        alarms:{create:()=>{},onAlarm:hook('alarm')},
        action:{setBadgeText:()=>{},setBadgeBackgroundColor:()=>{}},
        storage:{local:{
            get:(defaults,cb)=>cb({pending:[],queueOverflow:false}),
            set:(snapshot,cb)=>{
                stored.push(structuredClone(snapshot));
                events.length=0;
                events.push(...snapshot.pending);
                cb();
            }
        }}
    };
    const js=fs.readFileSync(
        path.join(__dirname,'..',browserName,'background.js'),'utf8');
    const sandbox={chrome,URL,console:{warn:()=>{}},setTimeout:()=>0,Date,Math,
        globalThis:{crypto:{randomUUID:()=>String(Math.random()).slice(2)}}};
    vm.runInNewContext(js,sandbox,{filename:browserName+'/background.js'});
    assert.equal(typeof listeners.committed,'function');
    listeners.committed({tabId:7,frameId:0,url:tab.url});
    const visit=events.find(e=>e.kind==='visit');
    assert.ok(visit,'Navigation must be buffered even when native host is absent');
    assert.equal(visit.host,'example.com');
    assert.equal(visit.browser,browserName);
    assert.equal(visit.url,'https://example.com/path?q=hello#section');
    assert.ok(!visit.url.includes('password') && !visit.url.includes('alice'));
    assert.equal(visit.title,'A Page Title');
    assert.ok(!('body' in visit));
    listeners.updated(7,{title:'Updated Page Title'},{...tab,
        url:'https://example.com/path?q=hello#section',
        title:'Updated Page Title'
    });
    const updated=events.find(e=>e.kind==='title');
    assert.ok(updated,'Title changes must be retained');
    assert.equal(updated.title,'Updated Page Title');
    assert.equal(updated.url,'https://example.com/path?q=hello#section');
    assert.ok(stored.length>=2,'Queue snapshots must be persisted');
}
run('firefox');
run('edge');
console.log('Browser URL/title metadata and missing-bridge isolation verified.');
