#!/usr/bin/env python3
"""
Temporal Dynamics Benchmark (TDB)
Tests what ACT-R actually does well: recency, frequency, importance, contradiction
"""

import json
import random
from dataclasses import dataclass, field
from datetime import datetime, timedelta
from pathlib import Path
from typing import Literal

@dataclass
class MemoryEvent:
    day: int
    memory: str
    importance: float = 0.5  # 0-1 scale

@dataclass
class TestCase:
    id: str
    category: Literal["recency_override", "frequency", "importance", "contradiction"]
    setup: list[MemoryEvent]
    query: str
    expected: str
    wrong: list[str]
    explanation: str = ""

@dataclass
class BenchmarkResults:
    system: str
    category: str
    total: int
    correct: int
    accuracy: float
    details: list[dict] = field(default_factory=list)


class TemporalBenchmarkGenerator:
    """Generate test cases for temporal dynamics evaluation"""
    
    def __init__(self, seed: int = 42):
        random.seed(seed)
        
    def generate_all(self, cases_per_category: int = 50) -> list[TestCase]:
        """Generate complete benchmark dataset"""
        cases = []
        cases.extend(self.generate_recency_override(cases_per_category))
        cases.extend(self.generate_frequency(cases_per_category))
        cases.extend(self.generate_importance(cases_per_category))
        cases.extend(self.generate_contradiction(cases_per_category))
        return cases
    
    def generate_recency_override(self, n: int = 50) -> list[TestCase]:
        """Cases where newer information should override older"""
        templates = [
            {
                "old": "I work at {company_old} as a {role}",
                "new": "I just started at {company_new}, excited for this new chapter",
                "query": "Where does the user work?",
                "expected_key": "company_new",
                "wrong_key": "company_old",
                "vars": [
                    {"company_old": "Google", "company_new": "Anthropic", "role": "engineer"},
                    {"company_old": "Meta", "company_new": "OpenAI", "role": "researcher"},
                    {"company_old": "Amazon", "company_new": "Stripe", "role": "developer"},
                    {"company_old": "Microsoft", "company_new": "Apple", "role": "PM"},
                    {"company_old": "Netflix", "company_new": "Spotify", "role": "data scientist"},
                ]
            },
            {
                "old": "My favorite programming language is {lang_old}",
                "new": "I've switched to {lang_new}, it's so much better for what I do",
                "query": "What programming language does the user prefer?",
                "expected_key": "lang_new",
                "wrong_key": "lang_old",
                "vars": [
                    {"lang_old": "Python", "lang_new": "Rust"},
                    {"lang_old": "JavaScript", "lang_new": "TypeScript"},
                    {"lang_old": "Java", "lang_new": "Kotlin"},
                    {"lang_old": "Ruby", "lang_new": "Go"},
                    {"lang_old": "PHP", "lang_new": "Python"},
                ]
            },
            {
                "old": "I live in {city_old}",
                "new": "Just finished moving to {city_new}, still unpacking",
                "query": "Where does the user live?",
                "expected_key": "city_new",
                "wrong_key": "city_old",
                "vars": [
                    {"city_old": "San Francisco", "city_new": "Seattle"},
                    {"city_old": "New York", "city_new": "Austin"},
                    {"city_old": "Boston", "city_new": "Denver"},
                    {"city_old": "Chicago", "city_new": "Miami"},
                    {"city_old": "Los Angeles", "city_new": "Portland"},
                ]
            },
            {
                "old": "I'm currently reading {book_old}",
                "new": "Just started {book_new}, couldn't put down {book_old}",
                "query": "What book is the user currently reading?",
                "expected_key": "book_new",
                "wrong_key": "book_old",
                "vars": [
                    {"book_old": "Dune", "book_new": "Project Hail Mary"},
                    {"book_old": "1984", "book_new": "Brave New World"},
                    {"book_old": "The Hobbit", "book_new": "The Name of the Wind"},
                    {"book_old": "Atomic Habits", "book_new": "Deep Work"},
                    {"book_old": "Sapiens", "book_new": "Homo Deus"},
                ]
            },
            {
                "old": "My main hobby is {hobby_old}",
                "new": "I've gotten really into {hobby_new} lately, {hobby_old} took a backseat",
                "query": "What is the user's main hobby?",
                "expected_key": "hobby_new",
                "wrong_key": "hobby_old",
                "vars": [
                    {"hobby_old": "photography", "hobby_new": "pottery"},
                    {"hobby_old": "running", "hobby_new": "rock climbing"},
                    {"hobby_old": "gaming", "hobby_new": "woodworking"},
                    {"hobby_old": "cooking", "hobby_new": "baking"},
                    {"hobby_old": "chess", "hobby_new": "go"},
                ]
            },
        ]
        
        cases = []
        case_id = 0
        while len(cases) < n:
            for template in templates:
                if len(cases) >= n:
                    break
                for var_set in template["vars"]:
                    if len(cases) >= n:
                        break
                    case_id += 1
                    old_day = random.randint(1, 10)
                    new_day = random.randint(15, 25)
                    cases.append(TestCase(
                        id=f"recency_{case_id:03d}",
                        category="recency_override",
                        setup=[
                            MemoryEvent(day=old_day, memory=template["old"].format(**var_set)),
                            MemoryEvent(day=new_day, memory=template["new"].format(**var_set)),
                        ],
                        query=template["query"],
                        expected=var_set[template["expected_key"]],
                        wrong=[var_set[template["wrong_key"]]],
                        explanation=f"Day {new_day} info should override day {old_day}"
                    ))
        return cases[:n]
    
    def generate_frequency(self, n: int = 50) -> list[TestCase]:
        """Cases where frequently mentioned items should rank higher"""
        templates = [
            {
                "rare": "Tried {food_rare} today, it was fine",
                "frequent": [
                    "Had {food_freq} for dinner",
                    "{food_freq} again, can't get enough",
                    "My usual {food_freq} order",
                    "{food_freq} night!",
                    "Craving {food_freq} as always",
                ],
                "query": "What food does the user like most?",
                "expected_key": "food_freq",
                "wrong_key": "food_rare",
                "vars": [
                    {"food_rare": "sushi", "food_freq": "pizza"},
                    {"food_rare": "Thai food", "food_freq": "tacos"},
                    {"food_rare": "Indian curry", "food_freq": "burgers"},
                    {"food_rare": "Greek salad", "food_freq": "pasta"},
                    {"food_rare": "Korean BBQ", "food_freq": "ramen"},
                ]
            },
            {
                "rare": "Watched {movie_rare} last night, interesting",
                "frequent": [
                    "Rewatching {movie_freq} again",
                    "{movie_freq} never gets old",
                    "Showed {movie_freq} to a friend",
                    "{movie_freq} marathon this weekend",
                    "Still thinking about {movie_freq}",
                ],
                "query": "What's the user's favorite movie?",
                "expected_key": "movie_freq",
                "wrong_key": "movie_rare",
                "vars": [
                    {"movie_rare": "Tenet", "movie_freq": "The Matrix"},
                    {"movie_rare": "Arrival", "movie_freq": "Interstellar"},
                    {"movie_rare": "Parasite", "movie_freq": "The Dark Knight"},
                    {"movie_rare": "Dune", "movie_freq": "Lord of the Rings"},
                    {"movie_rare": "Everything Everywhere", "movie_freq": "Inception"},
                ]
            },
            {
                "rare": "Tried {activity_rare} once, it was okay",
                "frequent": [
                    "{activity_freq} session today",
                    "Great {activity_freq} workout",
                    "{activity_freq} is my stress relief",
                    "Morning {activity_freq} routine",
                    "{activity_freq} with friends",
                ],
                "query": "What exercise does the user do regularly?",
                "expected_key": "activity_freq",
                "wrong_key": "activity_rare",
                "vars": [
                    {"activity_rare": "swimming", "activity_freq": "running"},
                    {"activity_rare": "yoga", "activity_freq": "weightlifting"},
                    {"activity_rare": "cycling", "activity_freq": "CrossFit"},
                    {"activity_rare": "tennis", "activity_freq": "basketball"},
                    {"activity_rare": "hiking", "activity_freq": "rock climbing"},
                ]
            },
        ]
        
        cases = []
        case_id = 0
        while len(cases) < n:
            for template in templates:
                if len(cases) >= n:
                    break
                for var_set in template["vars"]:
                    if len(cases) >= n:
                        break
                    case_id += 1
                    events = [MemoryEvent(day=1, memory=template["rare"].format(**var_set))]
                    for i, freq_template in enumerate(template["frequent"]):
                        events.append(MemoryEvent(
                            day=3 + i * 3,
                            memory=freq_template.format(**var_set)
                        ))
                    cases.append(TestCase(
                        id=f"frequency_{case_id:03d}",
                        category="frequency",
                        setup=events,
                        query=template["query"],
                        expected=var_set[template["expected_key"]],
                        wrong=[var_set[template["wrong_key"]]],
                        explanation=f"{var_set[template['expected_key']]} mentioned 5x vs 1x"
                    ))
        return cases[:n]
    
    def generate_importance(self, n: int = 50) -> list[TestCase]:
        """Cases where important memories should persist despite age"""
        templates = [
            {
                "important": "I'm severely allergic to {allergen}, I carry an EpiPen",
                "trivial": "Had a {food} for lunch",
                "query": "Does the user have any food allergies?",
                "expected_key": "allergen",
                "vars": [
                    {"allergen": "peanuts", "food": "sandwich"},
                    {"allergen": "shellfish", "food": "salad"},
                    {"allergen": "dairy", "food": "soup"},
                    {"allergen": "gluten", "food": "wrap"},
                    {"allergen": "tree nuts", "food": "burger"},
                ],
                "importance": 1.0,
            },
            {
                "important": "My daughter's birthday is {date}, she'll be {age}",
                "trivial": "Picked up some {item} from the store",
                "query": "When is the user's daughter's birthday?",
                "expected_key": "date",
                "vars": [
                    {"date": "March 15th", "age": "7", "item": "groceries"},
                    {"date": "July 4th", "age": "10", "item": "milk"},
                    {"date": "December 25th", "age": "5", "item": "bread"},
                    {"date": "October 31st", "age": "8", "item": "eggs"},
                    {"date": "February 14th", "age": "6", "item": "coffee"},
                ],
                "importance": 0.9,
            },
            {
                "important": "My social security number is {ssn_partial}",
                "trivial": "Weather was {weather} today",
                "query": "What's the user's SSN?",
                "expected_key": "ssn_partial",
                "vars": [
                    {"ssn_partial": "XXX-XX-1234", "weather": "nice"},
                    {"ssn_partial": "XXX-XX-5678", "weather": "rainy"},
                    {"ssn_partial": "XXX-XX-9012", "weather": "cloudy"},
                    {"ssn_partial": "XXX-XX-3456", "weather": "sunny"},
                    {"ssn_partial": "XXX-XX-7890", "weather": "cold"},
                ],
                "importance": 1.0,
            },
            {
                "important": "My emergency contact is {contact} at {phone}",
                "trivial": "Watched some {show} tonight",
                "query": "Who is the user's emergency contact?",
                "expected_key": "contact",
                "vars": [
                    {"contact": "Mom", "phone": "555-1234", "show": "TV"},
                    {"contact": "Sarah", "phone": "555-5678", "show": "Netflix"},
                    {"contact": "John", "phone": "555-9012", "show": "YouTube"},
                    {"contact": "Dad", "phone": "555-3456", "show": "movies"},
                    {"contact": "Amy", "phone": "555-7890", "show": "documentaries"},
                ],
                "importance": 0.95,
            },
        ]
        
        cases = []
        case_id = 0
        while len(cases) < n:
            for template in templates:
                if len(cases) >= n:
                    break
                for var_set in template["vars"]:
                    if len(cases) >= n:
                        break
                    case_id += 1
                    cases.append(TestCase(
                        id=f"importance_{case_id:03d}",
                        category="importance",
                        setup=[
                            MemoryEvent(
                                day=5,
                                memory=template["important"].format(**var_set),
                                importance=template["importance"]
                            ),
                            MemoryEvent(
                                day=28,
                                memory=template["trivial"].format(**var_set),
                                importance=0.2
                            ),
                        ],
                        query=template["query"],
                        expected=var_set[template["expected_key"]],
                        wrong=[],
                        explanation="Important memory should persist despite being older"
                    ))
        return cases[:n]
    
    def generate_contradiction(self, n: int = 50) -> list[TestCase]:
        """Cases with direct contradictions where latest should win"""
        templates = [
            {
                "states": [
                    ("I'm single, focusing on my career", "Single"),
                    ("Started dating someone new", "Dating"),
                    ("We got engaged last weekend!", "Engaged"),
                ],
                "query": "What's the user's relationship status?",
            },
            {
                "states": [
                    ("Working as a junior developer", "Junior developer"),
                    ("Got promoted to senior!", "Senior developer"),
                    ("Now I'm the tech lead", "Tech lead"),
                ],
                "query": "What's the user's current job title?",
            },
            {
                "states": [
                    ("I don't have any pets", "No pets"),
                    ("Just adopted a cat named Luna", "Has a cat"),
                    ("Got a dog too, Max is adorable", "Has cat and dog"),
                ],
                "query": "Does the user have pets?",
            },
            {
                "states": [
                    ("I'm a meat lover, steak every week", "Meat eater"),
                    ("Trying vegetarian this month", "Vegetarian"),
                    ("Going fully vegan now", "Vegan"),
                ],
                "query": "What's the user's diet?",
            },
            {
                "states": [
                    ("Renting an apartment downtown", "Renting"),
                    ("Just bought a condo!", "Condo owner"),
                    ("Moved to a house in the suburbs", "House in suburbs"),
                ],
                "query": "What's the user's living situation?",
            },
        ]
        
        cases = []
        case_id = 0
        while len(cases) < n:
            for template in templates:
                if len(cases) >= n:
                    break
                case_id += 1
                events = []
                wrong = []
                for i, (memory, label) in enumerate(template["states"]):
                    events.append(MemoryEvent(day=1 + i * 10, memory=memory))
                    if i < len(template["states"]) - 1:
                        wrong.append(label)
                
                cases.append(TestCase(
                    id=f"contradiction_{case_id:03d}",
                    category="contradiction",
                    setup=events,
                    query=template["query"],
                    expected=template["states"][-1][1],
                    wrong=wrong,
                    explanation="Latest state should override all previous"
                ))
        return cases[:n]
    
    def save(self, cases: list[TestCase], path: str):
        """Save benchmark to JSON"""
        data = {
            "version": "1.0",
            "generated": datetime.now().isoformat(),
            "total_cases": len(cases),
            "categories": {
                "recency_override": len([c for c in cases if c.category == "recency_override"]),
                "frequency": len([c for c in cases if c.category == "frequency"]),
                "importance": len([c for c in cases if c.category == "importance"]),
                "contradiction": len([c for c in cases if c.category == "contradiction"]),
            },
            "cases": [
                {
                    "id": c.id,
                    "category": c.category,
                    "setup": [{"day": e.day, "memory": e.memory, "importance": e.importance} for e in c.setup],
                    "query": c.query,
                    "expected": c.expected,
                    "wrong": c.wrong,
                    "explanation": c.explanation,
                }
                for c in cases
            ]
        }
        Path(path).write_text(json.dumps(data, indent=2))
        print(f"Saved {len(cases)} cases to {path}")


if __name__ == "__main__":
    generator = TemporalBenchmarkGenerator(seed=42)
    cases = generator.generate_all(cases_per_category=50)
    
    print(f"\nGenerated {len(cases)} total test cases:")
    for cat in ["recency_override", "frequency", "importance", "contradiction"]:
        count = len([c for c in cases if c.category == cat])
        print(f"  - {cat}: {count}")
    
    # Save to file
    generator.save(cases, "benchmarks/temporal_benchmark.json")
    
    # Show a few examples
    print("\n=== Sample Cases ===\n")
    for cat in ["recency_override", "frequency", "importance", "contradiction"]:
        case = next(c for c in cases if c.category == cat)
        print(f"[{case.category}] {case.id}")
        print(f"  Setup:")
        for e in case.setup:
            print(f"    Day {e.day}: {e.memory[:60]}...")
        print(f"  Query: {case.query}")
        print(f"  Expected: {case.expected}")
        print(f"  Wrong: {case.wrong}")
        print()
