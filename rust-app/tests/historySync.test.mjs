import {test} from 'node:test';
import assert from 'node:assert/strict';
import {createRefreshQueue,historySelection} from '../src/historySync.ts';

test('a new match preserves both explicit and implicit visible detail',()=>{
  const previous=[{id:'old-first'},{id:'old-second'}];
  const next=[{id:'new'},...previous];
  assert.deepEqual(historySelection(previous,next,''),{selectedId:'old-first',newRecordId:'new'});
  assert.deepEqual(historySelection(previous,next,'old-second'),{selectedId:'old-second',newRecordId:'new'});
  assert.deepEqual(historySelection([],next,''),{selectedId:'new',newRecordId:''});
  assert.deepEqual(historySelection(previous,previous,'old-second'),{selectedId:'old-second',newRecordId:''});
});

test('commits during an in-flight query are coalesced into one follow-up query',async()=>{
  let finish;
  let reads=0;
  const applied=[];
  const refresh=createRefreshQueue(()=>{reads++;return new Promise(resolve=>{finish=resolve;});},value=>applied.push(value));
  const first=refresh();
  await Promise.resolve();
  const second=refresh();
  refresh();
  assert.equal(reads,1);
  assert.equal(first,second);
  finish(['before-commit']);
  await Promise.resolve();
  assert.equal(reads,2);
  finish(['after-commit']);
  await first;
  assert.deepEqual(applied,[['before-commit'],['after-commit']]);
});

test('a failed query can be retried without clearing existing history',async()=>{
  let fail=true;
  const applied=[];
  const refresh=createRefreshQueue(async()=>{if(fail)throw Error('database busy');return ['recovered'];},value=>applied.push(value));
  await assert.rejects(refresh(),/database busy/);
  assert.deepEqual(applied,[]);
  fail=false;
  await refresh();
  assert.deepEqual(applied,[['recovered']]);
});
