import openai
import json
import time
import discord
import random
import asyncio

# Modules
from modules.module_logs import ModuleLogs
from modules.memories import MemoriesAPI
from modules.web_search import WebAPI
from modules.series_api import SeriesAPI
from modules.movies_api import MoviesAPI
from modules.examples import ExamplesAPI

# API credentials
credentials = json.loads(open("credentials.json").read())

openai.api_key = credentials["openai"]

Memories = MemoriesAPI(credentials["openai"])
WebSearch = WebAPI(credentials["openai"])

sonarr_url = credentials["sonarr"]["url"]
sonarr_headers = {
    "X-Api-Key": credentials["sonarr"]["api"],
    "Content-Type": "application/json",
}
sonarr_auth = (credentials["sonarr"]["authuser"], credentials["sonarr"]["authpass"])
Sonarr = SeriesAPI(credentials["openai"], sonarr_url, sonarr_headers, sonarr_auth)

radarr_url = credentials["radarr"]["url"]
radarr_headers = {
    "X-Api-Key": credentials["radarr"]["api"],
    "Content-Type": "application/json",
}
radarr_auth = (credentials["radarr"]["authuser"], credentials["radarr"]["authpass"])
Radarr = MoviesAPI(credentials["openai"], radarr_url, radarr_headers, radarr_auth)

Logs = ModuleLogs("main")
LogsReview = ModuleLogs("review")
LogsRelevance = ModuleLogs("relevance")

Examples = ExamplesAPI(credentials["openai"])


# Init messages
initMessages = [
    {
        "role": "user",
        "content": "You are media management assistant called CineMatic, enthusiastic, knowledgeable and passionate about all things media; always run lookups to ensure correct id, do not rely on chat history, if the data you have received does not contain what you need, you reply with the truthful answer of unknown, responses should all be on one line and compact language",
    },
    {
        "role": "user",
        "content": f"The current date is {time.strftime('%d/%m/%Y')}, the current time is {time.strftime('%H:%M:%S')}, if needing data beyond 2021 training data you can use a web search",
    },
]


async def runChatCompletion(
    botsMessage,
    botsStartMessage: str,
    usersName: str,
    usersId: str,
    message: list,
    relevantExamples: list,
    depth: int = 0,
) -> None:
    # Get the chat query to enter
    chatQuery = initMessages.copy()
    chatQuery += relevantExamples

    # Calculate tokens of the messages, GPT-3.5-Turbo has max tokens 4,096
    tokens = 0
    # Add up tokens in chatQuery
    for msg in chatQuery:
        tokens += len(msg["content"]) / 4 * 1.01
    # Add up tokens in message, but only add until limit is reached then remove earliest messages
    wantedMessages = []
    for msg in reversed(message):
        # Add token per 4 characters, give slight extra to make sure the limit is never reached
        tokens += len(msg["content"]) / 4 * 1.01
        # Token limit reached, stop adding messages
        if tokens > 4000:
            break
        # Add message to start of wantedMessages
        wantedMessages.insert(0, msg)
    message = wantedMessages

    # Run a chat completion
    response = openai.ChatCompletion.create(
        model="gpt-4", messages=chatQuery + message, temperature=0.7
    )
    # Log the response
    Logs.log("thread", json.dumps(chatQuery + message, indent=4), "", response)

    responseMessage = (
        response["choices"][0]["message"]["content"].replace("\n", " ").strip()
    )
    responseToUser = responseMessage[:]

    # Extract commands from the response, commands are within [], everything outside of [] is a response to the user
    commands = []
    hasCmdRet = False
    hasCmd = False
    while "[" in responseToUser:
        commands.append(
            responseToUser[responseToUser.find("[") + 1 : responseToUser.find("]")]
        )
        if "CMDRET" in commands[-1]:
            hasCmdRet = True
        elif "CMD" in commands[-1]:
            hasCmd = True
        responseToUser = (
            responseToUser.replace("[" + commands[-1] + "]", "")
            .replace("  ", " ")
            .strip()
        )

    message.append({"role": "assistant", "content": responseMessage})

    # Respond to user
    if len(responseToUser) > 0:
        # Add message into the botsMessage, emoji to show the message is in progress
        isntFinal = hasCmdRet and depth < 3
        await botsMessage.edit(
            content=botsStartMessage + (isntFinal and "⌛ " or "✅ ") + responseToUser
        )

    # Execute commands and return responses
    if hasCmdRet:
        returnMessage = ""
        for command in commands:
            command = command.split("~")
            if command[1] == "web_search":
                returnMessage += "[RES~" + await WebSearch.advanced(command[2]) + "]"
            elif command[1] == "movie_lookup":
                # If multiple terms, split and search for each
                if len(command[2].split("¬")) > 1:
                    for term in command[2].split("¬"):
                        returnMessage += (
                            "[RES~" + await Radarr.lookup_movie(term, command[3]) + "]"
                        )
                else:
                    returnMessage += (
                        "[RES~"
                        + await Radarr.lookup_movie(command[2], command[3])
                        + "]"
                    )
            elif command[1] == "series_lookup":
                # If multiple terms, split and search for each
                if len(command[2].split("¬")) > 1:
                    for term in command[2].split("¬"):
                        returnMessage += (
                            "[RES~" + await Sonarr.lookup_series(term, command[3]) + "]"
                        )
                else:
                    returnMessage += (
                        "[RES~"
                        + await Sonarr.lookup_series(command[2], command[3])
                        + "]"
                    )
            elif command[1] == "memory_get":
                returnMessage += (
                    "[RES~"
                    + await Memories.get_memory(usersName, usersId, command[2])
                    + "]"
                )

        message.append({"role": "system", "content": returnMessage})

        if depth < 3:
            runChatCompletion(
                botsMessage,
                botsStartMessage,
                usersName,
                usersId,
                message,
                relevantExamples,
                depth + 1,
            )
    # Execute regular commands
    elif hasCmd:
        for command in commands:
            command = command.split("~")
            if command[1] == "movie_post":
                Radarr.add_movie(command[2], command[3])
            # elif command[1] == 'movie_delete':
            #     Radarr.delete_movie(command[2])
            elif command[1] == "movie_put":
                Radarr.put_movie(command[2])
            elif command[1] == "series_post":
                Sonarr.add_series(command[2], command[3])
            # elif command[1] == 'series_delete':
            #     Sonarr.delete_series(command[2])
            elif command[1] == "series_put":
                Sonarr.put_series(command[2])
            elif command[1] == "memory_update":
                Memories.update_memory(usersName, usersId, command[2])


class MyClient(discord.Client):
    """Discord bot client class"""

    async def on_message(self, message):
        """Event handler for when a message is sent in a channel the bot has access to"""

        # Don't reply to ourselves
        if message.author.id == self.user.id:
            return

        # Check if message mentions bot
        mentionsBot = False
        if message.mentions:
            for mention in message.mentions:
                if mention.id == self.user.id:
                    mentionsBot = True
                    break
        if not mentionsBot:
            return

        # Check if message is a reply to the bot, if it is, create a message history
        messageHistory = []
        if message.reference is not None:
            replied_to = await message.channel.fetch_message(
                message.reference.message_id
            )
            if replied_to.author.id == self.user.id:
                # See if the message is completed
                if "✅" not in replied_to.content:
                    return
                # Split message by lines
                content = replied_to.content.split("\n")
                for msg in content:
                    # If the line is a reply to the bot, add it to the message history
                    if msg.startswith("✅"):
                        messageHistory.append(
                            {
                                "role": "assistant",
                                "content": msg.replace("✅ ", "☑️ ").strip(),
                            }
                        )
                    elif msg.startswith("☑️"):
                        messageHistory.append(
                            {
                                "role": "assistant",
                                "content": msg.strip(),
                            }
                        )
                    # If the line is a reply to the user, add it to the message history
                    elif msg.startswith("💬"):
                        messageHistory.append(
                            {
                                "role": "user",
                                "content": msg.strip(),
                            }
                        )

        # Get users id and name
        usersId = str(message.author.id)
        usersName = message.author.name
        print("Message from " + usersName + " (" + usersId + "): " + message.content)
        # Get message content, removing mentions and newlines
        userText = (
            message.content.replace("\n", " ")
            .replace("<@" + str(self.user.id) + ">", "")
            .strip()
        )

        # Reply to message
        replyMessage = [
            "Hey there! Super excited to process your message, give me just a moment... 🎬",
            "Oh, a message! Can't wait to dive into this one - I'm on it... 🎥",
            "Hey, awesome! A new message to explore! Let me work my media magic... 📺",
            "Woo-hoo! A fresh message to check out! Let me put my CineMatic touch on it... 🍿",
            "Yay, another message! Time to unleash my media passion, be right back... 📼",
            "Hey, a message! I'm so excited to process this one, just a moment... 🎞",
            "Aha! A message has arrived! Let me roll out the red carpet for it... 🎞️",
            "Ooh, a new message to dissect! Allow me to unleash my inner film buff... 🎦",
            "Lights, camera, action! Time to process your message with a cinematic twist... 📽️",
            "Hooray, a message to dig into! Let's make this a blockbuster experience... 🌟",
            "Greetings! Your message has caught my eye, let me give it the star treatment... 🎟️",
            "Popcorn's ready! Let me take a closer look at your message like a true film fanatic... 🍿",
            "Woohoo! A message to analyze! Let me work on it while humming my favorite movie tunes... 🎶",
            "A new message to dive into! Let me put on my director's hat and get to work... 🎩",
            "And... action! Time to process your message with my media expertise... 📹",
            "Hold on to your seats! I'm about to process your message with the excitement of a movie premiere... 🌆",
            "Sending your message to the cutting room! Let me work on it like a skilled film editor... 🎞️",
            "A message has entered the scene! Let me put my media prowess to work on it... 🎭",
            "Your message is the star of the show! Let me process it with the passion of a true cinephile... 🌟",
            "In the spotlight! Let me process your message with the enthusiasm of a film festival enthusiast... 🎪",
            "Curtain up! Your message takes center stage, and I'm ready to give it a standing ovation... 🎦",
        ]
        if message == None:
            return
        botsStartMessage = ""
        for msg in messageHistory:
            botsStartMessage += msg["content"] + "\n"
        botsStartMessage += f"💬 {userText}\n"
        botsMessage = await message.reply(
            f"{botsStartMessage}⌛ {random.choice(replyMessage)}"
        )

        # Get relevant examples, combine user text with message history
        userTextHistory = ""
        for message in messageHistory:
            if message["role"] == "user":
                userTextHistory += message["content"] + "\n"

        # Don't reply to non media queries
        messages = [
            {
                "role": "system",
                "content": "You determine if a users message is irrelevant to you, is it related to movies, series, asking for recommendations, changing resolution, adding or removing media etc? You reply with a single word answer, yes or no. If you are unsure respond with no.",
            }
        ]
        messages.append(
            {
                "role": "user",
                "content": f"{userTextHistory + userText}\nDo not respond to the above message, is the above text irrelevant, reply with a single word answer?",
            },
        )
        response = openai.ChatCompletion.create(
            model="gpt-4",
            messages=messages,
            temperature=0.7,
        )
        LogsRelevance.log("check", userTextHistory + userText, "", response)
        # If the ai responsed with yes, say I am a media bot
        print(
            "Is irrelevant? "
            + response["choices"][0]["message"]["content"].replace("\n", " ")
        )
        if response["choices"][0]["message"]["content"].lower().startswith("yes"):
            await botsMessage.edit(
                content=f"{botsStartMessage}❌ Hi, I'm a media bot. I can help you with media related questions. What would you like to know or achieve?"
            )
            return

        relevantExamples = await Examples.get_examples(userTextHistory + userText)
        # Get current messages
        currentMessage = []
        currentMessage.append({"role": "user", "content": f"Hi my name is {usersName}"})
        currentMessage.append(
            {"role": "assistant", "content": f"Hi, how can I help you?"}
        )
        # Add message history
        for message in messageHistory:
            currentMessage.append(message)
        # Add users message
        currentMessage.append({"role": "user", "content": userText})

        # Run chat completion async
        asyncio.create_task(
            runChatCompletion(
                botsMessage,
                botsStartMessage,
                usersName,
                usersId,
                currentMessage,
                relevantExamples,
                0,
            )
        )

    async def on_raw_reaction_add(self, payload):
        """When you thumbs down a bots message, it submits it for manual review"""

        channel = self.get_channel(payload.channel_id)
        message = await channel.fetch_message(payload.message_id)

        # If message is not from bot, do nothing
        if message.author.id != self.user.id:
            return
        # If message is not completed, or already submitted, do nothing
        if not message.content.startswith("✅") or "❗" in message.content:
            return
        # If reaction emoji is not thumbs down, do nothing
        if payload.emoji.name != "👎":
            return

        # Submit message for manual review
        LogsReview.log_simple(message.content)
        await message.edit(
            content=message.content
            + "\n❗ This message has been submitted for manual review."
        )

    async def on_ready(self):
        # Set the bot status to watching x movie or series or listening to x soundtrack every 10 minutes
        while True:
            response = openai.ChatCompletion.create(
                model="gpt-3.5-turbo",
                messages=[
                    {
                        "role": "user",
                        "content": "Give me a random movie or tv series, just the title nothing else, doesn't need to be popular",
                    },
                ],
                temperature=0.7,
            )
            activityName = response["choices"][0]["message"]["content"].strip()
            # If wrapped in quotes, remove them
            if activityName.startswith('"') and activityName.endswith('"'):
                activityName = activityName[1:-1]
            await self.change_presence(
                status=discord.Status.online,
                activity=discord.Activity(
                    type=discord.ActivityType.watching,
                    name=activityName,
                ),
            )
            await asyncio.sleep(6000)


intents = discord.Intents.default()
intents.message_content = True

client = MyClient(intents=intents)
client.run(credentials["discord"])
