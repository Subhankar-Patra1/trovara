import {config,samples} from './content.js';
import {Navigation,Hero,Workflow,FileTypes,FormatDetails,Privacy,CodeSearch,Comparison,FAQ,Download,Footer,Dialogs,SetupStage,icon,sampleMedia,escape} from './components-v2.js';

document.getElementById('site').innerHTML=Navigation()+`<main id="main" tabindex="-1">${Hero()}${FileTypes()}${Workflow()}${Privacy()}${CodeSearch()}${Comparison()}${FAQ()}${Download()}</main>`+Footer()+Dialogs();
const $=id=>document.getElementById(id);
const motion=matchMedia('(prefers-reduced-motion: reduce)');
const systemTheme=matchMedia('(prefers-color-scheme: dark)');

// Theme choice follows the system until the visitor selects Light or Dark.
function applyTheme(mode){
  const theme=mode==='system'?(systemTheme.matches?'dark':'light'):mode;
  document.documentElement.dataset.themeMode=mode;
  document.documentElement.dataset.theme=theme;
  document.documentElement.style.colorScheme=theme;
  document.querySelector('meta[name="theme-color"]').content=theme==='dark'?'#151517':'#f7f7f4';
  document.querySelectorAll('[data-theme-choice]').forEach(b=>b.setAttribute('aria-pressed',String(b.dataset.themeChoice===mode)));
  $('theme-toggle').setAttribute('aria-label',`Change theme. Current setting: ${mode}.`);
  try{localStorage.setItem('trovara-theme',mode);}catch{}
}
applyTheme(document.documentElement.dataset.themeMode||'system');
function closeTheme(returnFocus=false){$('theme-menu').hidden=true;$('theme-toggle').setAttribute('aria-expanded','false');if(returnFocus)$('theme-toggle').focus();}
$('theme-toggle').addEventListener('click',()=>{const open=$('theme-menu').hidden;closeProduct();closeMobile();$('theme-menu').hidden=!open;$('theme-toggle').setAttribute('aria-expanded',String(open));if(open)$('theme-menu').querySelector('[aria-pressed=true]')?.focus();});
document.querySelectorAll('[data-theme-choice]').forEach(b=>b.addEventListener('click',()=>{applyTheme(b.dataset.themeChoice);closeTheme(true);}));
systemTheme.addEventListener('change',()=>{if(document.documentElement.dataset.themeMode==='system')applyTheme('system');});
document.addEventListener('click',e=>{if(!e.target.closest('.theme-control'))closeTheme();});
$('theme-menu').addEventListener('keydown',e=>{
 if(e.key==='Escape'){e.preventDefault();closeTheme(true);return;}
 if(!['ArrowDown','ArrowUp','Home','End'].includes(e.key))return;
 e.preventDefault();const buttons=[...$('theme-menu').querySelectorAll('button')];let i=buttons.indexOf(document.activeElement);
 i=e.key==='Home'?0:e.key==='End'?buttons.length-1:(i+(e.key==='ArrowDown'?1:-1)+buttons.length)%buttons.length;buttons[i].focus();
});
$('theme-toggle').addEventListener('keydown',e=>{if(e.key==='Escape')closeTheme(true);});
document.addEventListener('focusin',e=>{if(!e.target.closest('.theme-control'))closeTheme();});

// Product navigation uses links in a disclosure panel.
function closeProduct(returnFocus=false){$('product-menu').hidden=true;$('product-toggle').setAttribute('aria-expanded','false');if(returnFocus)$('product-toggle').focus();}
function openProduct(){closeTheme();$('product-menu').hidden=false;$('product-toggle').setAttribute('aria-expanded','true');}
$('product-toggle').addEventListener('click',()=>{if($('product-menu').hidden)openProduct();else closeProduct();});
$('product-toggle').addEventListener('keydown',e=>{if(['ArrowDown','ArrowUp'].includes(e.key)){e.preventDefault();openProduct();const links=$('product-menu').querySelectorAll('a');links[e.key==='ArrowUp'?links.length-1:0].focus();}});
$('product-menu').addEventListener('click',e=>{if(e.target.closest('a'))closeProduct();});
document.addEventListener('click',e=>{if(!e.target.closest('.product-nav'))closeProduct();});
document.addEventListener('focusin',e=>{if(!e.target.closest('.product-nav'))closeProduct();});
document.querySelector('.product-nav').addEventListener('keydown',e=>{
 if(e.key==='Escape'){e.preventDefault();closeProduct(true);return;}
 if(e.target===$('product-toggle')||!['ArrowDown','ArrowUp','Home','End'].includes(e.key))return;
 e.preventDefault();const links=[...$('product-menu').querySelectorAll('a')];let i=links.indexOf(document.activeElement);
 i=e.key==='Home'?0:e.key==='End'?links.length-1:(i+(e.key==='ArrowDown'?1:-1)+links.length)%links.length;links[i].focus();
});
matchMedia('(max-width:1000px)').addEventListener('change',e=>{if(e.matches)closeProduct();});

// The small-screen menu keeps all sections and actions available.
function closeMobile(){ $('mobile-nav').hidden=true;$('mobile-menu-toggle').setAttribute('aria-expanded','false');$('mobile-menu-toggle').setAttribute('aria-label','Open navigation'); }
$('mobile-menu-toggle').addEventListener('click',()=>{const open=$('mobile-nav').hidden;closeTheme();$('mobile-nav').hidden=!open;$('mobile-menu-toggle').setAttribute('aria-expanded',String(open));$('mobile-menu-toggle').setAttribute('aria-label',open?'Close navigation':'Open navigation');});
$('mobile-nav').addEventListener('click',e=>{if(e.target.closest('a'))closeMobile();});
document.addEventListener('keydown',e=>{if(e.key==='Escape'&&!$('mobile-nav').hidden){closeMobile();$('mobile-menu-toggle').focus();}});
matchMedia('(min-width:1001px)').addEventListener('change',e=>{if(e.matches)closeMobile();});
document.addEventListener('click',e=>{if(!e.target.closest('.nav-shell'))closeMobile();});
const navObserver=new IntersectionObserver(entries=>{
 const visible=entries.filter(entry=>entry.isIntersecting).sort((a,b)=>a.boundingClientRect.top-b.boundingClientRect.top)[0];
 if(!visible)return;
 document.querySelectorAll('[data-nav-section]').forEach(link=>{if(link.dataset.navSection===visible.target.id)link.setAttribute('aria-current','location');else link.removeAttribute('aria-current');});
},{rootMargin:'-15% 0px -60% 0px',threshold:0});
['file-types','how-it-works','compare','faq'].forEach(id=>navObserver.observe($(id)));

// The web demo searches the six supplied examples. It never reads visitor files.
let currentSample=samples[0],typeTimer=null,settleTimer=null,queryTimer=null;
function stopTyping(){clearTimeout(typeTimer);clearTimeout(settleTimer);$('search-demo').classList.remove('is-searching');}
function animateQuery(text){
  stopTyping();if(motion.matches){$('demo-query').value=text;return;}
  let i=0;$('demo-query').value='';$('search-demo').classList.add('is-searching');
  const type=()=>{if(document.hidden){$('demo-query').value=text;stopTyping();return;}$('demo-query').value=text.slice(0,++i);if(i<text.length)typeTimer=setTimeout(type,25);else settleTimer=setTimeout(stopTyping,400);};
  typeTimer=setTimeout(type,60);
}
function pauseMedia(root){root.querySelectorAll('audio,video').forEach(media=>media.pause());}
function bindMedia(root){
 root.querySelectorAll('audio,video').forEach(media=>{
  if(media.dataset.bound)return;media.dataset.bound='true';
  if(media.dataset.start)media.addEventListener('loadedmetadata',()=>{media.currentTime=Math.min(Number(media.dataset.start)||0,Math.max(0,media.duration-.3));},{once:true});
  media.addEventListener('play',()=>{document.querySelectorAll('audio,video').forEach(other=>{if(other!==media)other.pause();});});
 });
}
function renderDemo(sample,{query=true,animate=false,message=null}={}){
  currentSample=sample;pauseMedia($('demo-asset'));
  $('demo-asset').innerHTML=sampleMedia(sample);
  $('demo-group').textContent=sample.group;
  $('demo-file').textContent=sample.name;
  $('demo-path').textContent=sample.path;
  $('demo-preview-name').textContent=sample.name;
  $('demo-match').innerHTML=icon('check')+escape(sample.reason);
  $('demo-reason').textContent=sample.description;
  $('demo-thumb').innerHTML=['image','screenshot','document','video'].includes(sample.id)?`<img src="${sample.asset}" alt="" width="34" height="37">`:icon(sample.icon);
  $('selected-result').setAttribute('aria-label',`Preview ${sample.name}`);
  $('selected-result').hidden=false;
  $('open-preview').disabled=false;
  document.querySelector('.results-heading>span:last-child').textContent='1 result';
  document.querySelectorAll('[data-sample]').forEach(b=>{const active=b.dataset.sample===sample.id;b.classList.toggle('active',active);b.setAttribute('aria-pressed',String(active));});
  $('demo-feedback').textContent=message||`Sample query: “${sample.query}”`;
  if(query){if(animate)animateQuery(sample.query);else{stopTyping();$('demo-query').value=sample.query;}}
  bindMedia($('demo-asset'));
}
document.querySelectorAll('[data-sample]').forEach(b=>b.addEventListener('click',()=>renderDemo(samples.find(s=>s.id===b.dataset.sample),{animate:true})));
function sampleFor(query){
 const q=query.trim().toLowerCase();
 if(!q)return null;
 if(/code:|token|function|auth|typescript/.test(q))return samples[5];
 if(/teacher|exam|recording|audio|lecture|class/.test(q))return samples[4];
 if(/video|lake|scene|moment/.test(q))return samples[3];
 if(/deposit|lease|document|pdf|rent/.test(q))return samples[2];
 if(/payment|declin|checkout|screenshot|error/.test(q))return samples[1];
 if(/red|car|photo|image|picture/.test(q))return samples[0];
 return null;
}
function searchSamples(){
 const sample=sampleFor($('demo-query').value);
 if(sample){renderDemo(sample,{query:false,message:'Sample match. Select Preview or press Enter to inspect it.'});return sample;}
 currentSample=null;pauseMedia($('demo-asset'));
 $('demo-asset').innerHTML=`<div class="empty-sample">${icon('search')}<strong>Try one of the sample files.</strong><p>For example: a red car, security deposit,<br>or the scene at the lake.</p></div>`;
 $('selected-result').hidden=true;$('demo-preview-name').textContent='Sample search';$('demo-match').textContent='Choose an example below';$('demo-reason').textContent='This website demonstrates six fictional sample files. Install Trovara to search your own selected folders.';
 $('demo-group').textContent='SAMPLE FILES';document.querySelector('.results-heading>span:last-child').textContent='0 results';$('open-preview').disabled=true;
 document.querySelectorAll('[data-sample]').forEach(b=>{b.classList.remove('active');b.setAttribute('aria-pressed','false');});
 $('demo-feedback').textContent='This demo searches the supplied examples only.';return null;
}
$('demo-query').addEventListener('input',()=>{stopTyping();clearTimeout(queryTimer);if(!motion.matches)$('search-demo').classList.add('is-searching');queryTimer=setTimeout(()=>{searchSamples();settleTimer=setTimeout(stopTyping,300);},180);});
$('demo-query').addEventListener('blur',stopTyping);
$('demo-form').addEventListener('submit',e=>{e.preventDefault();stopTyping();clearTimeout(queryTimer);const sample=searchSamples();if(sample)openPreview(sample);});
$('search-demo').addEventListener('keydown',e=>{
 if(!['ArrowLeft','ArrowRight'].includes(e.key)||e.target.matches('input,audio,video'))return;
 e.preventDefault();const index=samples.indexOf(currentSample);const next=(index+(e.key==='ArrowRight'?1:-1)+samples.length)%samples.length;renderDemo(samples[next],{animate:true});document.querySelector(`[data-sample="${samples[next].id}"]`).focus();
});

// File-type tabs support pointer, keyboard, and assistive technology navigation.
let activeFeature=samples[0];
function selectFileType(sample){
 activeFeature=sample;
 document.querySelector('.file-explorer').dataset.kind=sample.id;
 $('file-panel').innerHTML=FormatDetails(sample);
 document.querySelectorAll('[data-file-tab]').forEach(b=>{const active=b.dataset.fileTab===sample.id;b.classList.toggle('active',active);b.setAttribute('aria-selected',String(active));b.tabIndex=active?0:-1;});
 $('file-panel').setAttribute('aria-labelledby','tab-'+sample.id);
}
document.querySelectorAll('[data-file-tab]').forEach(b=>b.addEventListener('click',()=>selectFileType(samples.find(s=>s.id===b.dataset.fileTab))));
document.querySelector('.file-type-tabs').addEventListener('keydown',e=>{
 if(!['ArrowLeft','ArrowRight','ArrowDown','ArrowUp','Home','End'].includes(e.key))return;
 e.preventDefault();const tabs=[...document.querySelectorAll('[data-file-tab]')];let i=tabs.indexOf(document.activeElement);
 i=e.key==='Home'?0:e.key==='End'?tabs.length-1:(i+(['ArrowRight','ArrowDown'].includes(e.key)?1:-1)+tabs.length)%tabs.length;
 selectFileType(samples.find(s=>s.id===tabs[i].dataset.fileTab));tabs[i].focus();
});

function trySample(sample){renderDemo(sample);$('search-demo').scrollIntoView({behavior:motion.matches?'instant':'smooth',block:'center'});$('demo-query').focus({preventScroll:true});}
$('try-feature').addEventListener('click',()=>trySample(activeFeature));
document.querySelectorAll('[data-try-sample]').forEach(b=>b.addEventListener('click',()=>trySample(samples.find(s=>s.id===b.dataset.trySample))));



// Setup illustrations preserve the visitor's sample folder selection.
const folderSelection=[true,true,true];
function updateFolderCount(){const count=folderSelection.filter(Boolean).length;if($('setup-selection'))$('setup-selection').textContent=`${count} sample folder${count===1?'':'s'} selected`;}
function selectStep(step){
 document.querySelectorAll('[data-step]').forEach(b=>{const active=Number(b.dataset.step)===step;b.classList.toggle('active',active);b.setAttribute('aria-selected',String(active));b.tabIndex=active?0:-1;});
 $('setup-stage').innerHTML=SetupStage(step);$('setup-stage').setAttribute('aria-labelledby',`step-tab-${step}`);
 $('setup-stage').querySelectorAll('input[type=checkbox]').forEach((input,i)=>input.checked=folderSelection[i]);updateFolderCount();
}
$('setup-stage').addEventListener('change',e=>{if(e.target.matches('input[type=checkbox]')){const inputs=[...$('setup-stage').querySelectorAll('input[type=checkbox]')];folderSelection[inputs.indexOf(e.target)]=e.target.checked;updateFolderCount();}});
document.querySelectorAll('[data-step]').forEach(b=>b.addEventListener('click',()=>selectStep(Number(b.dataset.step))));
document.querySelector('.setup-steps').addEventListener('keydown',e=>{
 if(!['ArrowUp','ArrowDown','ArrowLeft','ArrowRight','Home','End'].includes(e.key))return;
 e.preventDefault();const tabs=[...document.querySelectorAll('[data-step]')];let i=tabs.indexOf(document.activeElement);
 i=e.key==='Home'?0:e.key==='End'?tabs.length-1:(i+(['ArrowRight','ArrowDown'].includes(e.key)?1:-1)+tabs.length)%tabs.length;
 selectStep(i);tabs[i].focus();
});

// The product video has one player. Chapter controls live inside its dialog.
const productVideo=$('product-video');
let chapterRequest=0;
async function playChapter(time){
 const request=++chapterRequest;
 if(productVideo.readyState<1){productVideo.load();await new Promise(resolve=>{productVideo.addEventListener('loadedmetadata',resolve,{once:true});productVideo.addEventListener('error',resolve,{once:true});});}
 if(request!==chapterRequest||productVideo.error||!$('video-dialog').open)return;
 productVideo.currentTime=Math.min(time,Math.max(0,productVideo.duration-.1));
 productVideo.play().catch(()=>{});
}
document.querySelectorAll('[data-chapter]').forEach(button=>button.addEventListener('click',()=>playChapter(Number(button.dataset.chapter))));

// The local-flow animation runs only after a button press.
let flowTimers=[];
function stopFlow(){flowTimers.forEach(clearTimeout);flowTimers=[];delete document.querySelector('.privacy-diagram').dataset.flow;$('replay-local-flow').disabled=false;}
$('replay-local-flow').addEventListener('click',()=>{
 stopFlow();const diagram=document.querySelector('.privacy-diagram');
 if(motion.matches){$('diagram-status').textContent='Selected files form a local index. Trovara compares your query and returns a matching file on this PC.';return;}
 $('replay-local-flow').disabled=true;
 const stages=[['files','Trovara reads the selected files on this PC.'],['core','Trovara compares the query with the local index.'],['result','The matching file appears in the search results.']];
 stages.forEach(([stage,text],i)=>{const update=()=>{diagram.dataset.flow=stage;$('diagram-status').textContent=text;};if(i===0)update();else flowTimers.push(setTimeout(update,i*850));});
 flowTimers.push(setTimeout(()=>{stopFlow();$('diagram-status').textContent='Files and search stay inside this PC.';},2850));
});

document.querySelectorAll('[data-faq-filter]').forEach(button=>button.addEventListener('click',()=>{
 const selected=button.dataset.faqFilter;let count=0;
 document.querySelectorAll('[data-faq-filter]').forEach(b=>b.setAttribute('aria-pressed',String(b===button)));
 document.querySelectorAll('[data-faq-group]').forEach(group=>{group.hidden=selected!=='all'&&group.dataset.faqGroup!==selected;if(!group.hidden)count+=group.querySelectorAll('details').length;});
 $('faq-filter-status').textContent=`Showing ${count} ${selected==='all'?'':selected+' '}questions.`;
}));

// Native dialogs supply focus containment and Escape-to-close behavior.
let dialogTrigger=null;
function openDialog(dialog){dialogTrigger=document.activeElement;dialog.showModal();document.body.style.overflow='hidden';}
function openPreview(sample){
 if(!sample)return;
 $('preview-title').textContent=sample.name;$('modal-preview').innerHTML=sampleMedia(sample,{large:true});$('preview-description').textContent=sample.caption+' Fictional sample file.';
 bindMedia($('modal-preview'));openDialog($('preview-dialog'));
}
$('selected-result').addEventListener('click',()=>openPreview(currentSample));
$('open-preview').addEventListener('click',()=>openPreview(currentSample));
document.querySelectorAll('[data-watch]').forEach(b=>b.addEventListener('click',()=>{pauseMedia(document);openDialog($('video-dialog'));$('product-video').play().catch(()=>{});}));
document.querySelectorAll('[data-close-dialog]').forEach(b=>b.addEventListener('click',()=>b.closest('dialog').close()));
document.querySelectorAll('dialog').forEach(dialog=>{
 dialog.addEventListener('click',e=>{if(e.target===dialog){const rect=dialog.getBoundingClientRect();if(e.clientX<rect.left||e.clientX>rect.right||e.clientY<rect.top||e.clientY>rect.bottom)dialog.close();}});
 dialog.addEventListener('close',()=>{pauseMedia(dialog);document.body.style.overflow='';if(dialog.id==='preview-dialog')$('modal-preview').replaceChildren();dialogTrigger?.focus();});
});
document.addEventListener('visibilitychange',()=>{if(document.hidden){stopTyping();stopFlow();pauseMedia(document);}});
motion.addEventListener('change',()=>{if(motion.matches){stopTyping();stopFlow();if(currentSample)$('demo-query').value=currentSample.query;}});
bindMedia(document);

// Use a published installer from this repository. Keep a working release link otherwise.
async function updateRelease(){
 const controller=new AbortController(),timeout=setTimeout(()=>controller.abort(),6500);
 try{
  const response=await fetch(config.releaseAPI,{headers:{Accept:'application/vnd.github+json'},signal:controller.signal});
  if(!response.ok)throw new Error('Release lookup unavailable');
  const releases=await response.json();
  const release=Array.isArray(releases)?releases.find(r=>!r.draft&&r.assets?.some(a=>/setup.*\.exe$|-setup\.exe$/i.test(a.name))):null;
  const installer=release?.assets.find(a=>/setup.*\.exe$|-setup\.exe$/i.test(a.name));
  if(installer?.browser_download_url?.startsWith(config.repository+'/releases/download/')){
   const label=`Download ${release.tag_name} for Windows`;
   $('release-button').href=installer.browser_download_url;$('release-button').innerHTML=icon('windows')+`<span>${escape(label)}</span>`+icon('down');
   $('release-note').textContent=`${(installer.size/1e6).toFixed(1)} MB installer${release.prerelease?' · Prerelease':''}. The 865 MB model downloads separately during setup.`;
   const published=release.published_at?new Date(release.published_at).toLocaleDateString(undefined,{year:'numeric',month:'long',day:'numeric'}):'';
   $('latest-release').textContent=`Latest: ${release.tag_name}${published?`, released ${published}`:''}.`;
   if(release.html_url?.startsWith(config.repository+'/releases/'))$('latest-release-link').href=release.html_url;
  }else $('release-note').textContent='No Windows installer is published yet. Check GitHub for release updates.';
 }catch{$('release-note').textContent='Check GitHub for Windows installer availability and release notes.';}
 finally{clearTimeout(timeout);}
}
updateRelease();
