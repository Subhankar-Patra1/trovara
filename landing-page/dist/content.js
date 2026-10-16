export const config = {
  repository: 'https://github.com/Subhankar-Patra1/trovara',
  releases: 'https://github.com/Subhankar-Patra1/trovara/releases',
  releaseAPI: 'https://api.github.com/repos/Subhankar-Patra1/trovara/releases?per_page=10',
};

export const samples = [
  {id:'image',label:'Images',icon:'image',color:'green',group:'IMAGES',query:'a red car',name:'DSC_1001.png',path:'C:\\Demo\\Pictures',
    badge:'Visual match',asset:'assets/red-car.webp',alt:'A red car parked beside a lake with forested hills.',
    reason:'Matches the picture',description:'Find a photo by its subject. The filename does not need to contain your search words.',
    caption:'A red car beside a lake. Saved as DSC_1001.png.',formats:['JPG','PNG','HEIC','WebP','RAW'],
    title:'The photo you remember.',detail:'Describe a subject, a place, or an object. Trovara compares your description with the image content.',
    note:'Some image formats require an installed Windows codec.'},
  {id:'screenshot',label:'Screenshots',icon:'scan',color:'blue',group:'SCREENSHOTS',query:'payment declined',name:'Screenshot 2026-10-14.png',path:'C:\\Demo\\Screenshots',
    badge:'Text match',asset:'assets/payment.webp',alt:'Fictional checkout screenshot showing the highlighted words Payment declined.',
    reason:'Matches the text on screen',description:'Find the screenshot by the words it contains. Windows OCR reads supported text in the image.',
    caption:'The words “Payment declined” are highlighted in the screenshot.',formats:['PNG','JPG','WebP','Windows OCR'],
    title:'The screenshot with those words.',detail:'Search a message, an error, or a phrase you saw. The preview highlights the matching text.',
    note:'Text recognition depends on the Windows OCR language packs installed on your PC.'},
  {id:'document',label:'Documents',icon:'document',color:'blue',group:'DOCUMENTS',query:'security deposit',name:'Sample_Lease.pdf',path:'C:\\Demo\\Documents',
    badge:'Page 3 of 8',asset:'assets/lease.webp',alt:'A fictional lease passage with security deposit highlighted.',
    reason:'Matches the text on page 3',description:'Find the relevant passage inside a document. The result keeps the matching page number.',
    caption:'“Security deposit” appears on page 3 of the sample lease.',formats:['PDF','DOCX','XLSX','PPTX','Markdown','Email'],
    title:'The page with the answer.',detail:'Search document text, slide text, spreadsheet cells, and saved email. Preview the matching passage before opening the file.',
    note:'Page navigation in an external application depends on that application’s support.'},
  {id:'video',label:'Video',icon:'video',color:'red',group:'VIDEO',query:'the scene at the lake',name:'Weekend_Trip.mp4',path:'C:\\Demo\\Videos',
    badge:'0:05',asset:'assets/lake.webp',alt:'A lake shore with green trees in the sample video.',
    reason:'Matches the frame at 0:05',description:'Find a scene in a video. Preview the matching frame, then play from that moment.',
    caption:'A lake scene at 0:05 in Weekend_Trip.mp4.',formats:['MP4','MOV','MKV','AVI','Subtitles'],
    title:'The moment in the video.',detail:'Trovara indexes sampled frames, audio segments, and supported subtitles. Results keep their timestamps.',
    note:'Video indexing samples frames. Playback support depends on the file format and available codecs.'},
  {id:'audio',label:'Audio',icon:'audio',color:'lilac',group:'AUDIO',query:'what the teacher said about exams',name:'Class_Notes.wav',path:'C:\\Demo\\Recordings',
    badge:'0:30',asset:'assets/class-notes.wav',alt:'Fictional classroom recording about an exam.',
    reason:'Matches what is heard at 0:30',description:'Find a topic inside a recording. Listen from the matching timestamp.',
    caption:'“The exam is on Friday. Review chapters three and four.”',formats:['MP3','WAV','M4A','FLAC','OGG'],
    title:'The part you remember hearing.',detail:'Search a topic from a lecture, voice note, or recording. Trovara compares your query with indexed audio segments.',
    note:'The sample player starts at 0:30. Select Play to hear the fictional classroom excerpt.'},
  {id:'code',label:'Code',icon:'code',color:'orange',group:'CODE',query:'code: refresh the access token',name:'auth.ts',path:'demo-app\\src',
    badge:'Lines 1–14',asset:'',alt:'A TypeScript function that refreshes an access token.',
    reason:'Matches the function’s purpose',description:'Find source code by what it does, or by an exact identifier. Open the matching line in a supported editor.',
    caption:'refreshAccessToken() in the fictional demo-app project.',formats:['TypeScript','Python','Rust','C/C++','Notebooks'],
    title:'The function that does it.',detail:'Enable the separate code index and select your project folders. Use code: followed by a description or identifier.',
    note:'Code search reads source files. It does not run or modify your code.'},
];

export const code = `export async function refreshAccessToken(token: string) {
  const response = await fetch("/api/auth/refresh", {
    method: "POST",
    body: JSON.stringify({ refreshToken: token }),
  });

  if (!response.ok) {
    throw new Error("Token refresh failed");
  }

  const session = await response.json();
  return session.accessToken;
}`;

export const faqs = [
 ['What does Trovara search?', 'Trovara searches supported images, screenshots, PDFs, documents, audio, videos, and optional source-code folders. You choose which folders it indexes.'],
 ['Do my files leave my PC?', 'Your files, search queries, and indexes stay on your PC. Model downloads, update checks, and requested add-ons use the internet.'],
 ['Does it work offline?', 'Yes. After the model download, indexing and search work offline. You can disable update checks in Settings.'],
 ['Why does setup download about 865 MB?', 'That is the local EmbeddingGemma 2 model: about 310 MB for text and 555 MB for the image and audio encoders. The installer is separate. Indexes and previews need additional disk space.'],
 ['Will indexing slow down my PC?', 'The first pass reads your selected files and uses CPU, disk, and a supported GPU. Later passes process new or changed files and unfinished content. You can pause indexing and adjust its limits in Settings.'],
 ['Do I need a GPU or an AI account?', 'No. Trovara can use the CPU. A supported Vulkan GPU can reduce indexing time. No paid AI account or API key is required.'],
 ['Which languages can I search?', 'EmbeddingGemma 2 supports more than 100 languages and searches across languages. Results vary by language. Windows OCR depends on installed language packs. Exact-word search does not split text into individual words for languages written without spaces.'],
 ['Does Trovara support macOS or Linux?', 'The current desktop app supports 64-bit Windows 10 (22H2) and Windows 11. This repository does not provide macOS or Linux releases.'],
 ['Can I search cloud folders?', 'Trovara can index locally downloaded files in selected cloud folders. It skips cloud-only placeholders and does not download them for you.'],
 ['How do I remove Trovara?', 'Quit Trovara from the tray and uninstall it from Windows Settings. Select the uninstaller’s application-data option to remove the model, indexes, settings, and logs. Your original files stay in their folders.'],
];
