// Headless TinyML UI smoke test using local fixture data only.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || "playwright");
const http = require("node:http");
const fs = require("node:fs");
const path = require("node:path");
const assert = require("node:assert/strict");
const root = path.resolve(__dirname, "../web");
const stamp = "2026-01-01T00:00:00Z";
const run = {id:"test-run",run_id:"wire-1",run_name:"Fixture run",device_id:"node",mode:"evaluation",status:"completed",samples:2,labelled_samples:2,accuracy:.5,mean_confidence:.8,mean_inference_ms:1.5,duration_seconds:2,started_at_server:stamp,model:{name:"Classifier",version:"1"}};
const rows = [1,2].map(n => ({sample_id:String(n),server_timestamp:stamp,actual_class:"A",predicted_class:n===1?"A":"B",confidence:.8,correct:n===1,inference_us:n*1000,total_pipeline_us:n*1200,chip_temperature_c:null,free_heap_bytes:10000,other:{nested:{signal:n},unsafe:'<img src=x onerror="window.injected=true">'}}));
const report = {generated_at:stamp,run,model:run.model,device:{board:"esp32s3"},classification:{samples:2,accuracy:.5},class_metrics:[{class:"A",support:2,precision:1,recall:.5,f1:2/3}],confusion_matrix:{labels:["A","B"],matrix:[[1,1],[0,0]]},embedded:{inference_ms:{mean:1.5,p95:1.95},thermal:{average_chip_temperature_c:null}},analysis:["1 misclassification from 2 labelled samples."],other:{custom:true}};

async function main() {
  let brokerConnected=true;
  const server=http.createServer((req,res) => {
    const name=new URL(req.url,"http://localhost").pathname;
    const file=path.resolve(root,name==="/"?"index.html":`.${name}`);
    if(!file.startsWith(`${root}${path.sep}`)){res.writeHead(403);res.end();return;}
    fs.readFile(file,(error,data) => {if(error){res.writeHead(404);res.end();return;}res.setHeader("Content-Type",file.endsWith(".js")?"text/javascript":file.endsWith(".css")?"text/css":"text/html");res.end(data);});
  });
  await new Promise(resolve => server.listen(0,"127.0.0.1",resolve));
  let browser;
  try {
    browser=await chromium.launch({headless:true});
    const page=await browser.newPage({viewport:{width:1440,height:900}}),errors=[];
    let secureRequests=0;
    page.on("pageerror",error => errors.push(error.message));
    await page.route("https://fonts.googleapis.com/**",route => route.abort());
    await page.route("**/sessions",route => route.fulfill({json:[]}));
    await page.route("**/api/**",async route => {
      const url=new URL(route.request().url()),p=url.pathname;
      if(p==="/api/ota/console/devices") {
        secureRequests++;
        if(route.request().headers().authorization!=="Bearer test-viewer-token") {
          await route.fulfill({status:401,json:{error:{message:"Invalid viewer token"}}});
        } else await route.fulfill({json:[{device_id:"node",name:"Test board"}]});
        return;
      }
      if(!p.startsWith("/api/tinyml")){await route.fulfill({json:p==="/api/ota/devices"?[{device_id:"node",name:"Test board"}]:p.endsWith("status")?{status:"online"}:[]});return;}
      secureRequests++;
      assert.equal(route.request().headers().authorization,"Bearer test-viewer-token");
      let value;
      if(p.endsWith("/status")) value={mqtt_enabled:true,mqtt_configured:true,mqtt_connected:brokerConnected,overview:{connected_devices:1,active_runs:0,completed_runs:1,total_inferences:2,accuracy:.5,mean_inference_ms:1.5}};
      else if(p.endsWith("/devices")) value=[{device_id:"node",name:"Test board"}];
      else if(p.endsWith("/runs")) value={items:[run],total:1};
      else if(p.endsWith("/results")) value={items:rows,total:2};
      else if(p.endsWith("/snapshots")||p.endsWith("/events")) value={items:[]};
      else if(p.endsWith("/report")||p.endsWith("/metrics")||p.endsWith("/report/json")) value=report;
      else if(p.endsWith("/results.csv")){await route.fulfill({contentType:"text/csv",body:"sample_id,confidence\r\n1,0.8\r\n"});return;}
      else value=run;
      await route.fulfill({json:value});
    });
    let reportingSocket;
    await page.routeWebSocket("**/api/tinyml/ws",ws => {
      reportingSocket=ws;
      ws.onMessage(() => ws.send(JSON.stringify({type:"tinyml.connected"})));
    });
    await page.routeWebSocket("**/api/ota/console/ws",ws => ws.onMessage(data => {
      const request=JSON.parse(data);
      assert.equal(request.token,"test-viewer-token");
      assert.equal(request.device_id,"node");
      ws.send(JSON.stringify({notice:"Authenticated; live stream ready"}));
    }));
    await page.goto(`http://127.0.0.1:${server.address().port}/?view=tinyml`);
    assert.equal(await page.locator("#tml-workspace").isVisible(),true);
    assert.equal(await page.locator("#viewer-access-panel").isVisible(),true);
    assert.equal(await page.locator("#viewer-access-connect").isDisabled(),true);
    assert.equal(await page.locator("#tml-device").isDisabled(),true);
    assert.deepEqual((await page.locator(".nav-subitem.active").allTextContents()).map(text => text.trim()),["Reporting"]);
    await page.locator("#nav-console").click();
    assert.equal(await page.locator("#console-workspace").isVisible(),true);
    assert.equal(await page.locator("#console-device").isDisabled(),true);
    assert.equal(await page.locator("#console-connection-badge").textContent(),"Access required");
    assert.equal(await page.locator("#console-refresh").isDisabled(),true);
    assert.equal(await page.locator("#console-connect").isDisabled(),true);
    assert.equal(secureRequests,0);
    await page.locator("#viewer-access-token").fill("wrong-token");
    assert.equal(await page.locator("#viewer-access-connect").isEnabled(),true);
    await page.locator("#viewer-access-connect").click();
    await page.locator("#viewer-access-status").filter({hasText:"incorrect"}).waitFor();
    assert.equal(await page.locator("#console-device").isDisabled(),true);
    await page.locator("#nav-tinyml").click();
    await page.locator("#nav-csv-toggle").click();
    assert.equal(await page.locator("#nav-csv-submenu").isHidden(),true);
    assert.equal(await page.locator("#nav-csv-toggle").getAttribute("aria-expanded"),"false");
    await page.locator("#nav-csv-toggle").click();
    assert.equal(await page.locator("#nav-csv-submenu").isVisible(),true);
    await page.locator("#nav-profiler").click();
    assert.equal(await page.locator("#nav-profiler").getAttribute("class"),"nav-subitem active");
    assert.equal(await page.locator("#nav-csv-toggle").getAttribute("class"),"nav-group-toggle active-parent");
    await page.locator("#nav-runs").click();
    assert.equal(await page.locator("#nav-runs").getAttribute("class"),"nav-subitem active");
    await page.locator("#nav-ota").click();
    assert.equal(await page.locator("#ota-service-indicator").count(),0);
    assert.equal(await page.locator("#ota-summary-service").count(),0);
    await page.locator("#ota-project-input").setInputFiles({name:"sensor-firmware.zip",mimeType:"application/zip",buffer:Buffer.from("fixture")});
    assert.equal(await page.locator("#ota-project-name").textContent(),"sensor-firmware.zip");
    assert.equal(await page.locator("#ota-project-file").isVisible(),true);
    await page.screenshot({path:path.resolve(__dirname,"../data/ota-upload-ui-test.png"),fullPage:true});
    await page.locator("#ota-project-remove").click();
    assert.equal(await page.locator("#ota-project-file").isHidden(),true);
    await page.locator("#ota-project-input").setInputFiles({name:"not-a-zip.txt",mimeType:"text/plain",buffer:Buffer.from("fixture")});
    assert.equal(await page.locator("#ota-project-error").textContent(),"Choose a .zip archive.");
    await page.locator("#nav-tinyml").click();
    await page.screenshot({path:path.resolve(__dirname,"../data/tinyml-ui-disconnected-test.png"),fullPage:true});
    await page.locator("#viewer-access-token").fill("test-viewer-token");
    await page.locator("#viewer-access-connect").click();
    await page.locator("#tml-device:enabled").waitFor();
    assert.equal(await page.locator("#viewer-access-token").isHidden(),true);
    await page.getByRole("button",{name:"Fixture run",exact:true}).click();
    await page.locator("#tml-report-state").filter({hasText:"Saved"}).waitFor();
    assert.equal(await page.locator("#tml-results>tr").count(),2);
    assert.equal(await page.locator("#tml-results img").count(),0);
    assert.equal(await page.evaluate(() => window.injected),undefined);
    await page.locator("#tml-signal").selectOption({label:"Other /nested/signal"});
    assert.equal(await page.locator("#tml-chart svg").count(),1);
    await page.screenshot({path:path.resolve(__dirname,"../data/tinyml-ui-test.png"),fullPage:true});
    await page.locator("#nav-console").click();
    await page.locator("#console-device:enabled").waitFor();
    assert.equal(await page.locator("#console-device").isEnabled(),true);
    assert.equal(await page.locator("#console-device option").count(),1);
    await page.locator("#console-connect").click();
    await page.locator("#console-connection-badge").filter({hasText:"Streaming"}).waitFor();
    await page.locator("#console-disconnect").click();
    assert.equal(await page.evaluate(() => window.viewerAccess.get()),"test-viewer-token");
    await page.locator("#viewer-access-disconnect").click();
    assert.equal(await page.evaluate(() => window.viewerAccess.get()),"");
    assert.equal(await page.locator("#console-device").isDisabled(),true);
    assert.equal(await page.locator("#console-connection-badge").textContent(),"Access required");
    await page.locator("#nav-tinyml").click();
    assert.equal(await page.locator("#tml-device").isDisabled(),true);
    assert.equal(await page.locator("#viewer-access-connect").isDisabled(),true);
    await page.locator("#nav-console").click();
    await page.locator("#viewer-access-token").fill("test-viewer-token");
    await page.locator("#viewer-access-connect").click();
    await page.locator("#console-device:enabled").waitFor();
    await page.locator("#console-connect").click();
    await page.locator("#console-connection-badge").filter({hasText:"Streaming"}).waitFor();
    await page.locator("#nav-tinyml").click();
    await page.locator("#tml-connection").filter({hasText:"Live"}).waitFor();
    brokerConnected=false;
    reportingSocket.send(JSON.stringify({type:"tinyml.broker.status",connected:false}));
    await page.locator("#tml-connection").filter({hasText:"Broker offline"}).waitFor();
    brokerConnected=true;
    reportingSocket.send(JSON.stringify({type:"tinyml.broker.status",connected:true}));
    await page.locator("#tml-connection").filter({hasText:"Live"}).waitFor();
    await page.locator("#viewer-access-disconnect").click();
    assert.equal(await page.evaluate(() => window.viewerAccess.get()),"");
    assert.equal(await page.locator("#console-device").isDisabled(),true);
    assert.equal(await page.locator("#console-connection-badge").textContent(),"Access required");
    await page.setViewportSize({width:390,height:844});
    assert.equal(await page.locator("#tinyml-section").isVisible(),true);
    assert.deepEqual(errors,[]);
    console.log("PASS: Reporting and Remote Console share one viewer session; report, chart, escaping, and responsive views");
  } finally {await browser?.close();await new Promise(resolve => server.close(resolve));}
}
main().catch(error => {console.error(error);process.exitCode=1;});
